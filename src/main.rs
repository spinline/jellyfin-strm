mod config;
mod generator;
mod jellyfin;

use anyhow::Result;
use config::Config;
use generator::StrmGenerator;
use jellyfin::JellyfinClient;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;
use tokio::time::{sleep, Duration};
use tracing::{error, info};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    info!("Starting Jellyfin STRM Generator v{}", env!("CARGO_PKG_VERSION"));

    let config = Config::load(None::<&str>)?;
    info!("Jellyfin URL: {}", config.jellyfin_url);
    info!("Output Directory: {:?}", config.output_dir);
    info!("Movies Directory: {}", config.movies_dir_name);
    info!("TV Shows Directory: {}", config.series_dir_name);
    info!("Download Images: {}", config.download_images);
    info!("Generate NFO: {}", config.generate_nfo);
    info!("Scan Interval: {} minutes", config.scan_interval_minutes);
    info!("Clean Deleted Files: {}", config.clean_deleted);

    let mut client = JellyfinClient::new(
        config.jellyfin_url.clone(),
        config.username.clone(),
        config.password.clone(),
        config.api_key.clone(),
    );

    loop {
        let start = Instant::now();
        info!(">>> Starting Jellyfin library scan & STRM synchronization...");

        match run_sync(&mut client, &config).await {
            Ok(total_files) => {
                info!(
                    ">>> Synchronization complete! Active STRM files: {} (elapsed: {:.2?})",
                    total_files,
                    start.elapsed()
                );
            }
            Err(e) => {
                error!(">>> Synchronization error: {:#}", e);
            }
        }

        if config.scan_interval_minutes == 0 {
            info!("Scan interval is 0; exiting after single run.");
            break;
        }

        info!(
            "Next scan scheduled in {} minutes. Waiting...",
            config.scan_interval_minutes
        );

        tokio::select! {
            _ = sleep(Duration::from_secs(config.scan_interval_minutes * 60)) => {}
            _ = tokio::signal::ctrl_c() => {
                info!("Shutdown signal received. Exiting gracefully.");
                break;
            }
        }
    }

    Ok(())
}

async fn run_sync(client: &mut JellyfinClient, config: &Config) -> Result<usize> {
    // Authenticate or refresh token
    client.authenticate().await?;

    let token = client
        .token
        .as_ref()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("Missing access token after authentication"))?;

    let generator = StrmGenerator::new(
        config.jellyfin_url.clone(),
        token,
        config.output_dir.clone(),
        config.clean_deleted,
        config.generate_nfo,
        config.download_images,
        config.image_concurrency,
    );

    let mut libraries = client.get_libraries().await?;
    info!("Found {} libraries on Jellyfin", libraries.len());

    // Sort libraries so movies are prioritized before TV series
    libraries.sort_by_key(|lib| {
        let col = lib.collection_type.as_deref().unwrap_or("");
        if col == "movies" || lib.name.to_lowercase().contains("film") || lib.name.to_lowercase().contains("movie") {
            0
        } else if col == "tvshows" || lib.name.to_lowercase().contains("dizi") || lib.name.to_lowercase().contains("show") || lib.name.to_lowercase().contains("series") {
            1
        } else {
            2
        }
    });

    let arc_client = Arc::new(JellyfinClient::new(
        client.base_url.clone(),
        None,
        None,
        client.token.clone(),
    ));

    let mut all_active_files = HashSet::new();

    for lib in libraries {
        if !config.library_filter.is_empty()
            && !config.library_filter.iter().any(|f| f.eq_ignore_ascii_case(&lib.name))
        {
            info!("Skipping library '{}' (not in library_filter)", lib.name);
            continue;
        }

        let col_type = lib.collection_type.as_deref().unwrap_or("");
        let is_movies = col_type == "movies"
            || (col_type.is_empty() && (lib.name.to_lowercase().contains("film") || lib.name.to_lowercase().contains("movie")));
        let is_series = col_type == "tvshows"
            || (col_type.is_empty() && (lib.name.to_lowercase().contains("dizi") || lib.name.to_lowercase().contains("show") || lib.name.to_lowercase().contains("series")));

        if is_movies {
            if !config.include_movies {
                continue;
            }
            let target_dir = if config.use_library_names {
                config.output_dir.join(generator::sanitize_folder(&lib.name))
            } else {
                config.output_dir.join(&config.movies_dir_name)
            };
            std::fs::create_dir_all(&target_dir)?;

            info!("Scanning movie library: '{}' -> {:?}", lib.name, target_dir);

            let mut start_index = 0;
            let page_size = 500;
            let mut processed = 0;

            loop {
                let (items, total) = client.get_movies_page(&lib.id, start_index, page_size).await?;
                let count = items.len();
                if count == 0 {
                    break;
                }
                generator
                    .generate_movie_batch(&items, &target_dir, &mut all_active_files, &arc_client)
                    .await?;
                processed += count;
                info!("[{}] Progress: {}/{} movies synced", lib.name, processed, total);

                if count < page_size || processed >= total {
                    break;
                }
                start_index += count;
            }
        } else if is_series {
            if !config.include_series {
                continue;
            }
            let target_dir = if config.use_library_names {
                config.output_dir.join(generator::sanitize_folder(&lib.name))
            } else {
                config.output_dir.join(&config.series_dir_name)
            };
            std::fs::create_dir_all(&target_dir)?;

            info!("Scanning series library: '{}' -> {:?}", lib.name, target_dir);

            // Sync series posters/metadata
            let mut series_start = 0;
            let series_page_size = 500;
            let mut series_processed = 0;
            loop {
                let (series_items, series_total) = client.get_series_page(&lib.id, series_start, series_page_size).await?;
                let count = series_items.len();
                if count == 0 {
                    break;
                }
                generator.sync_series_metadata(&series_items, &target_dir, &arc_client).await?;
                series_processed += count;
                info!("[{}] Series metadata progress: {}/{}", lib.name, series_processed, series_total);
                if count < series_page_size || series_processed >= series_total {
                    break;
                }
                series_start += count;
            }

            // Sync episodes
            let mut start_index = 0;
            let page_size = 500;
            let mut processed = 0;

            loop {
                let (items, total) = client.get_episodes_page(&lib.id, start_index, page_size).await?;
                let count = items.len();
                if count == 0 {
                    break;
                }
                generator.generate_episode_batch(&items, &target_dir, &mut all_active_files)?;
                processed += count;
                info!("[{}] Progress: {}/{} episodes synced", lib.name, processed, total);

                if count < page_size || processed >= total {
                    break;
                }
                start_index += count;
            }
        } else {
            info!("Ignoring library '{}' (type: '{}')", lib.name, col_type);
        }
    }

    if config.clean_deleted {
        let _ = generator.clean_orphans(&all_active_files)?;
    }

    Ok(all_active_files.len())
}
