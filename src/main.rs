mod config;
mod generator;
mod jellyfin;

use anyhow::Result;
use config::Config;
use generator::StrmGenerator;
use jellyfin::JellyfinClient;
use std::collections::HashSet;
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
    );

    let libraries = client.get_libraries().await?;
    info!("Found {} libraries on Jellyfin", libraries.len());

    let mut all_active_files = HashSet::new();

    for lib in libraries {
        if !config.library_filter.is_empty()
            && !config.library_filter.iter().any(|f| f.eq_ignore_ascii_case(&lib.name))
        {
            info!("Skipping library '{}' (not in library_filter)", lib.name);
            continue;
        }

        let col_type = lib.collection_type.as_deref().unwrap_or("");
        match col_type {
            "movies" => {
                if !config.include_movies {
                    continue;
                }
                info!("Scanning movie library: '{}' (ID: {})...", lib.name, lib.id);
                let target_dir = generator.target_library_dir(&lib.name);
                std::fs::create_dir_all(&target_dir)?;

                let mut start_index = 0;
                let page_size = 500;
                let mut processed = 0;

                loop {
                    let (items, total) = client.get_movies_page(&lib.id, start_index, page_size).await?;
                    let count = items.len();
                    if count == 0 {
                        break;
                    }
                    generator.generate_movie_batch(&items, &target_dir, &mut all_active_files)?;
                    processed += count;
                    info!("[{}] Progress: {}/{} movies synced", lib.name, processed, total);

                    if count < page_size || processed >= total {
                        break;
                    }
                    start_index += count;
                }
            }
            "tvshows" => {
                if !config.include_series {
                    continue;
                }
                info!("Scanning series library: '{}' (ID: {})...", lib.name, lib.id);
                let target_dir = generator.target_library_dir(&lib.name);
                std::fs::create_dir_all(&target_dir)?;

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
            }
            other => {
                // If collection_type is unknown/empty or mixed, check name
                if lib.name.to_lowercase().contains("film") || lib.name.to_lowercase().contains("movie") {
                    if config.include_movies {
                        info!("Scanning movie library by name: '{}' (ID: {})...", lib.name, lib.id);
                        let target_dir = generator.target_library_dir(&lib.name);
                        std::fs::create_dir_all(&target_dir)?;
                        let mut start_index = 0;
                        let page_size = 500;
                        let mut processed = 0;
                        loop {
                            let (items, total) = client.get_movies_page(&lib.id, start_index, page_size).await?;
                            let count = items.len();
                            if count == 0 {
                                break;
                            }
                            generator.generate_movie_batch(&items, &target_dir, &mut all_active_files)?;
                            processed += count;
                            info!("[{}] Progress: {}/{} movies synced", lib.name, processed, total);
                            if count < page_size || processed >= total {
                                break;
                            }
                            start_index += count;
                        }
                    }
                } else if lib.name.to_lowercase().contains("dizi") || lib.name.to_lowercase().contains("show") || lib.name.to_lowercase().contains("series") {
                    if config.include_series {
                        info!("Scanning series library by name: '{}' (ID: {})...", lib.name, lib.id);
                        let target_dir = generator.target_library_dir(&lib.name);
                        std::fs::create_dir_all(&target_dir)?;
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
                    }
                } else {
                    info!("Ignoring library '{}' (type: '{}')", lib.name, other);
                }
            }
        }
    }

    if config.clean_deleted {
        let _ = generator.clean_orphans(&all_active_files)?;
    }

    Ok(all_active_files.len())
}
