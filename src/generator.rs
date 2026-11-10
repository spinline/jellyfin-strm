use crate::jellyfin::{EpisodeItem, JellyfinClient, MovieItem, SeriesItem};
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Semaphore;
use tracing::{debug, info, warn};

pub struct StrmGenerator {
    base_url: String,
    token: String,
    pub output_dir: PathBuf,
    clean_deleted: bool,
    generate_nfo: bool,
    download_images: bool,
    image_concurrency: usize,
}

impl StrmGenerator {
    pub fn new(
        base_url: String,
        token: String,
        output_dir: PathBuf,
        clean_deleted: bool,
        generate_nfo: bool,
        download_images: bool,
        image_concurrency: usize,
    ) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            token,
            output_dir,
            clean_deleted,
            generate_nfo,
            download_images,
            image_concurrency,
        }
    }

    pub fn generate_movie_folder_name(name: &str, year: Option<u32>) -> String {
        if let Some(y) = year {
            if y > 0 {
                return format!("{} ({})", name, y);
            }
        }
        name.to_string()
    }

    pub async fn generate_movie_batch(
        &self,
        movies: &[MovieItem],
        target_dir: &Path,
        active_files: &mut HashSet<PathBuf>,
        client: &Arc<JellyfinClient>,
    ) -> Result<(usize, usize)> {
        let mut created_count = 0;
        let mut unchanged_count = 0;
        let mut image_tasks: Vec<(String, String, PathBuf)> = Vec::new();

        for movie in movies {
            let folder_name = Self::generate_movie_folder_name(&movie.name, movie.production_year);
            let movie_dir = target_dir.join(sanitize_folder(&folder_name));

            if let Err(e) = fs::create_dir_all(&movie_dir) {
                warn!("Failed to create folder {:?}: {}", movie_dir, e);
                continue;
            }

            // 1. Generate movie.nfo if enabled
            if self.generate_nfo {
                let nfo_path = movie_dir.join("movie.nfo");
                if !nfo_path.exists() {
                    let xml = generate_movie_nfo(movie);
                    if let Err(e) = fs::write(&nfo_path, xml) {
                        warn!("Failed to write {:?}: {}", nfo_path, e);
                    }
                }
                active_files.insert(nfo_path);
            }

            // 2. Queue images if enabled (only if file doesn't already exist on disk)
            if self.download_images {
                let poster_path = movie_dir.join("poster.jpg");
                active_files.insert(poster_path.clone());
                if !poster_path.exists() && movie.image_tags.contains_key("Primary") {
                    image_tasks.push((movie.id.clone(), "Primary".to_string(), poster_path));
                }

                let bg_path = movie_dir.join("background.jpg");
                active_files.insert(bg_path.clone());
                if !bg_path.exists() && !movie.backdrop_image_tags.is_empty() {
                    image_tasks.push((movie.id.clone(), "Backdrop".to_string(), bg_path));
                }

                let logo_path = movie_dir.join("logo.png");
                active_files.insert(logo_path.clone());
                if !logo_path.exists() && movie.image_tags.contains_key("Logo") {
                    image_tasks.push((movie.id.clone(), "Logo".to_string(), logo_path));
                }
            }

            // 3. Generate STRM files
            // Rule: If single version -> {folder_name}.strm (e.g. Interstellar (2014).strm)
            // Rule: If multiple versions/resolutions -> {real_basename_from_jellyfin}.strm
            if movie.media_sources.len() <= 1 {
                let file_name = format!("{}.strm", sanitize_file(&folder_name));
                let file_path = movie_dir.join(file_name);
                let stream_id = movie
                    .media_sources
                    .first()
                    .map(|ms| ms.id.as_str())
                    .unwrap_or(movie.id.as_str());

                let stream_url = format!(
                    "{}/Videos/{}/stream?static=true&api_key={}",
                    self.base_url, stream_id, self.token
                );

                if write_strm_if_changed(&file_path, &stream_url)? {
                    created_count += 1;
                } else {
                    unchanged_count += 1;
                }
                active_files.insert(file_path);
            } else {
                for ms in &movie.media_sources {
                    let base_name = ms
                        .path
                        .as_deref()
                        .or(movie.path.as_deref())
                        .and_then(extract_real_basename)
                        .unwrap_or_else(|| {
                            let label = ms.name.as_deref().unwrap_or("version");
                            format!("{} - {}", folder_name, label)
                        });

                    let file_name = format!("{}.strm", sanitize_file(&base_name));
                    let file_path = movie_dir.join(file_name);
                    let stream_url = format!(
                        "{}/Videos/{}/stream?static=true&api_key={}",
                        self.base_url, ms.id, self.token
                    );

                    if write_strm_if_changed(&file_path, &stream_url)? {
                        created_count += 1;
                    } else {
                        unchanged_count += 1;
                    }
                    active_files.insert(file_path);
                }
            }
        }

        // Execute concurrent image downloads for this batch
        if !image_tasks.is_empty() {
            download_images_concurrently(image_tasks, client, self.image_concurrency).await;
        }

        Ok((created_count, unchanged_count))
    }

    pub async fn sync_series_metadata(
        &self,
        series_items: &[SeriesItem],
        target_dir: &Path,
        client: &Arc<JellyfinClient>,
        series_folder_map: &mut HashMap<String, String>,
        active_files: &mut HashSet<PathBuf>,
    ) -> Result<()> {
        let mut image_tasks: Vec<(String, String, PathBuf)> = Vec::new();

        for series in series_items {
            let folder_name = Self::generate_movie_folder_name(&series.name, series.production_year);
            let sanitized = sanitize_folder(&folder_name);

            // Register in mapping
            series_folder_map.insert(series.id.clone(), sanitized.clone());
            series_folder_map.insert(series.name.to_lowercase(), sanitized.clone());

            let series_dir = target_dir.join(&sanitized);

            if let Err(e) = fs::create_dir_all(&series_dir) {
                warn!("Failed to create series folder {:?}: {}", series_dir, e);
                continue;
            }

            // tvshow.nfo
            if self.generate_nfo {
                let nfo_path = series_dir.join("tvshow.nfo");
                if !nfo_path.exists() {
                    let xml = generate_series_nfo(series);
                    if let Err(e) = fs::write(&nfo_path, xml) {
                        warn!("Failed to write {:?}: {}", nfo_path, e);
                    }
                }
                active_files.insert(nfo_path);
            }

            // Series images
            if self.download_images {
                let poster_path = series_dir.join("poster.jpg");
                active_files.insert(poster_path.clone());
                if !poster_path.exists() && series.image_tags.contains_key("Primary") {
                    image_tasks.push((series.id.clone(), "Primary".to_string(), poster_path));
                }

                let bg_path = series_dir.join("background.jpg");
                active_files.insert(bg_path.clone());
                if !bg_path.exists() && !series.backdrop_image_tags.is_empty() {
                    image_tasks.push((series.id.clone(), "Backdrop".to_string(), bg_path));
                }

                let logo_path = series_dir.join("logo.png");
                active_files.insert(logo_path.clone());
                if !logo_path.exists() && series.image_tags.contains_key("Logo") {
                    image_tasks.push((series.id.clone(), "Logo".to_string(), logo_path));
                }
            }
        }

        if !image_tasks.is_empty() {
            download_images_concurrently(image_tasks, client, self.image_concurrency).await;
        }

        Ok(())
    }

    pub async fn generate_episode_batch(
        &self,
        episodes: &[EpisodeItem],
        target_dir: &Path,
        series_folder_map: &HashMap<String, String>,
        active_files: &mut HashSet<PathBuf>,
        client: &Arc<JellyfinClient>,
    ) -> Result<(usize, usize)> {
        let mut created_count = 0;
        let mut unchanged_count = 0;
        let mut image_tasks: Vec<(String, String, PathBuf)> = Vec::new();
        let mut thumb_links: Vec<(PathBuf, PathBuf)> = Vec::new();

        for ep in episodes {
            let series_folder = ep
                .series_id
                .as_deref()
                .and_then(|sid| series_folder_map.get(sid))
                .or_else(|| {
                    ep.series_name
                        .as_deref()
                        .and_then(|sn| series_folder_map.get(&sn.to_lowercase()))
                })
                .map(|s| s.as_str())
                .unwrap_or_else(|| ep.series_name.as_deref().unwrap_or("Unknown Series"));

            let season_folder = match ep.parent_index_number {
                Some(0) => "Specials".to_string(),
                Some(s) => format!("Season {:02}", s),
                None => ep.season_name.as_deref().unwrap_or("Season 01").to_string(),
            };

            let episode_dir = target_dir
                .join(sanitize_folder(series_folder))
                .join(sanitize_folder(&season_folder));

            if let Err(e) = fs::create_dir_all(&episode_dir) {
                warn!("Failed to create folder {:?}: {}", episode_dir, e);
                continue;
            }

            let base_name = ep.path.as_deref().and_then(extract_real_basename).unwrap_or_else(|| {
                match (ep.parent_index_number, ep.index_number) {
                    (Some(s), Some(e)) => format!("{} - S{:02}E{:02} - {}", series_folder, s, e, ep.name),
                    (_, Some(e)) => format!("{} - E{:02} - {}", series_folder, e, ep.name),
                    _ => format!("{} - {}", series_folder, ep.name),
                }
            });

            let sanitized_base = sanitize_file(&base_name);

            // 1. STRM file
            let file_name = format!("{}.strm", sanitized_base);
            let file_path = episode_dir.join(file_name);
            let stream_id = ep
                .media_sources
                .first()
                .map(|ms| ms.id.as_str())
                .unwrap_or(ep.id.as_str());

            let stream_url = format!(
                "{}/Videos/{}/stream?static=true&api_key={}",
                self.base_url, stream_id, self.token
            );

            if write_strm_if_changed(&file_path, &stream_url)? {
                created_count += 1;
            } else {
                unchanged_count += 1;
            }
            active_files.insert(file_path);

            // 2. Episode NFO file
            if self.generate_nfo {
                let nfo_path = episode_dir.join(format!("{}.nfo", sanitized_base));
                if !nfo_path.exists() {
                    let xml = generate_episode_nfo(ep, series_folder);
                    if let Err(e) = fs::write(&nfo_path, xml) {
                        warn!("Failed to write episode NFO {:?}: {}", nfo_path, e);
                    }
                }
                active_files.insert(nfo_path);
            }

            // 3. Episode Image (Thumbnail)
            if self.download_images && ep.image_tags.contains_key("Primary") {
                let img_path = episode_dir.join(format!("{}.jpg", sanitized_base));
                let thumb_path = episode_dir.join(format!("{}-thumb.jpg", sanitized_base));

                active_files.insert(img_path.clone());
                active_files.insert(thumb_path.clone());

                if !img_path.exists() && !thumb_path.exists() {
                    image_tasks.push((ep.id.clone(), "Primary".to_string(), img_path.clone()));
                    thumb_links.push((img_path, thumb_path));
                } else if img_path.exists() && !thumb_path.exists() {
                    let _ = fs::hard_link(&img_path, &thumb_path);
                } else if !img_path.exists() && thumb_path.exists() {
                    let _ = fs::hard_link(&thumb_path, &img_path);
                }
            }
        }

        // Execute concurrent episode image downloads
        if !image_tasks.is_empty() {
            download_images_concurrently(image_tasks, client, self.image_concurrency).await;

            // Create hardlinks for thumb.jpg for instant Kodi/Plex compatibility with 0 extra bytes
            for (src, dst) in thumb_links {
                if src.exists() && !dst.exists() {
                    if let Err(_) = fs::hard_link(&src, &dst) {
                        let _ = fs::copy(&src, &dst);
                    }
                }
            }
        }

        Ok((created_count, unchanged_count))
    }

    pub fn clean_orphans(&self, active_files: &HashSet<PathBuf>) -> Result<usize> {
        if !self.clean_deleted || !self.output_dir.exists() {
            return Ok(0);
        }

        let mut deleted_count = 0;
        let mut all_strm_files = Vec::new();
        collect_strm_files(&self.output_dir, &mut all_strm_files)?;

        for file in all_strm_files {
            if !active_files.contains(&file) {
                if let Err(e) = fs::remove_file(&file) {
                    warn!("Failed to delete orphan strm file {:?}: {}", file, e);
                } else {
                    debug!("Deleted orphan strm: {:?}", file);
                    deleted_count += 1;

                    // Also remove accompanying .nfo, .jpg, -thumb.jpg if they exist
                    let stem = file.with_extension("");
                    let nfo = stem.with_extension("nfo");
                    let jpg = stem.with_extension("jpg");
                    let thumb = PathBuf::from(format!("{}-thumb.jpg", stem.to_string_lossy()));

                    let _ = fs::remove_file(nfo);
                    let _ = fs::remove_file(jpg);
                    let _ = fs::remove_file(thumb);
                }
            }
        }

        // Clean up empty directories and leftover metadata for deleted movies/shows
        clean_empty_and_orphaned_dirs(&self.output_dir, &self.output_dir);

        if deleted_count > 0 {
            info!("Cleaned up {} removed strm files from disk", deleted_count);
        }

        Ok(deleted_count)
    }
}

async fn download_images_concurrently(
    tasks: Vec<(String, String, PathBuf)>,
    client: &Arc<JellyfinClient>,
    concurrency: usize,
) {
    let sem = Arc::new(Semaphore::new(concurrency));
    let mut futures = Vec::new();

    for (item_id, img_type, dest_path) in tasks {
        let sem = sem.clone();
        let client = client.clone();
        futures.push(async move {
            let _permit = match sem.acquire().await {
                Ok(p) => p,
                Err(_) => return,
            };

            match client.download_image(&item_id, &img_type).await {
                Ok(Some(bytes)) => {
                    if let Err(e) = tokio::fs::write(&dest_path, &bytes).await {
                        warn!("Failed to save image {:?}: {}", dest_path, e);
                    } else {
                        debug!("Downloaded {} for {} -> {:?}", img_type, item_id, dest_path);
                    }
                }
                Ok(None) => {
                    debug!("Image {} not found (404) for item {}", img_type, item_id);
                }
                Err(e) => {
                    debug!("Error downloading {} for {}: {}", img_type, item_id, e);
                }
            }
        });
    }

    futures::future::join_all(futures).await;
}

pub fn generate_movie_nfo(movie: &MovieItem) -> String {
    let mut out = String::with_capacity(1024);
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n");
    out.push_str("<movie>\n");

    out.push_str(&format!("  <title>{}</title>\n", escape_xml(&movie.name)));
    if let Some(ref orig) = movie.original_title {
        out.push_str(&format!("  <originaltitle>{}</originaltitle>\n", escape_xml(orig)));
    } else {
        out.push_str(&format!("  <originaltitle>{}</originaltitle>\n", escape_xml(&movie.name)));
    }

    if let Some(year) = movie.production_year {
        if year > 0 {
            out.push_str(&format!("  <year>{}</year>\n", year));
        }
    }

    if let Some(ref date) = movie.premiere_date {
        let clean_date = date.split('T').next().unwrap_or(date);
        out.push_str(&format!("  <premiered>{}</premiered>\n", escape_xml(clean_date)));
    }

    if let Some(rating) = movie.community_rating {
        out.push_str(&format!("  <rating>{:.1}</rating>\n", rating));
    }

    if let Some(ref plot) = movie.overview {
        out.push_str(&format!("  <plot>{}</plot>\n", escape_xml(plot)));
        out.push_str(&format!("  <outline>{}</outline>\n", escape_xml(plot)));
    }

    if let Some(tagline) = movie.taglines.first() {
        out.push_str(&format!("  <tagline>{}</tagline>\n", escape_xml(tagline)));
    }

    if let Some(ticks) = movie.run_time_ticks {
        let mins = (ticks / 600_000_000).max(1);
        out.push_str(&format!("  <runtime>{}</runtime>\n", mins));
    }

    if let Some(ref mpaa) = movie.official_rating {
        out.push_str(&format!("  <mpaa>{}</mpaa>\n", escape_xml(mpaa)));
    }

    // Provider IDs
    if let Some(imdb) = movie.provider_ids.get("Imdb").or_else(|| movie.provider_ids.get("imdb")) {
        out.push_str(&format!("  <uniqueid type=\"imdb\" default=\"true\">{}</uniqueid>\n", escape_xml(imdb)));
        out.push_str(&format!("  <id>{}</id>\n", escape_xml(imdb)));
    }
    if let Some(tmdb) = movie.provider_ids.get("Tmdb").or_else(|| movie.provider_ids.get("tmdb")) {
        out.push_str(&format!("  <uniqueid type=\"tmdb\">{}</uniqueid>\n", escape_xml(tmdb)));
    }

    // Genres
    for g in &movie.genres {
        out.push_str(&format!("  <genre>{}</genre>\n", escape_xml(g)));
    }

    // Directors & Actors
    for person in &movie.people {
        if let Some(ref t) = person.person_type {
            if t.eq_ignore_ascii_case("Director") {
                out.push_str(&format!("  <director>{}</director>\n", escape_xml(&person.name)));
            } else if t.eq_ignore_ascii_case("Actor") {
                out.push_str("  <actor>\n");
                out.push_str(&format!("    <name>{}</name>\n", escape_xml(&person.name)));
                if let Some(ref r) = person.role {
                    out.push_str(&format!("    <role>{}</role>\n", escape_xml(r)));
                }
                out.push_str("  </actor>\n");
            }
        }
    }

    out.push_str("</movie>\n");
    out
}

pub fn generate_series_nfo(series: &SeriesItem) -> String {
    let mut out = String::with_capacity(1024);
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n");
    out.push_str("<tvshow>\n");

    out.push_str(&format!("  <title>{}</title>\n", escape_xml(&series.name)));
    if let Some(ref orig) = series.original_title {
        out.push_str(&format!("  <originaltitle>{}</originaltitle>\n", escape_xml(orig)));
    }

    if let Some(year) = series.production_year {
        if year > 0 {
            out.push_str(&format!("  <year>{}</year>\n", year));
        }
    }

    if let Some(ref date) = series.premiere_date {
        let clean_date = date.split('T').next().unwrap_or(date);
        out.push_str(&format!("  <premiered>{}</premiered>\n", escape_xml(clean_date)));
    }

    if let Some(rating) = series.community_rating {
        out.push_str(&format!("  <rating>{:.1}</rating>\n", rating));
    }

    if let Some(ref plot) = series.overview {
        out.push_str(&format!("  <plot>{}</plot>\n", escape_xml(plot)));
    }

    if let Some(tagline) = series.taglines.first() {
        out.push_str(&format!("  <tagline>{}</tagline>\n", escape_xml(tagline)));
    }

    if let Some(ref mpaa) = series.official_rating {
        out.push_str(&format!("  <mpaa>{}</mpaa>\n", escape_xml(mpaa)));
    }

    if let Some(imdb) = series.provider_ids.get("Imdb").or_else(|| series.provider_ids.get("imdb")) {
        out.push_str(&format!("  <uniqueid type=\"imdb\" default=\"true\">{}</uniqueid>\n", escape_xml(imdb)));
        out.push_str(&format!("  <id>{}</id>\n", escape_xml(imdb)));
    }
    if let Some(tmdb) = series.provider_ids.get("Tmdb").or_else(|| series.provider_ids.get("tmdb")) {
        out.push_str(&format!("  <uniqueid type=\"tmdb\">{}</uniqueid>\n", escape_xml(tmdb)));
    }
    if let Some(tvdb) = series.provider_ids.get("Tvdb").or_else(|| series.provider_ids.get("tvdb")) {
        out.push_str(&format!("  <uniqueid type=\"tvdb\">{}</uniqueid>\n", escape_xml(tvdb)));
    }

    for g in &series.genres {
        out.push_str(&format!("  <genre>{}</genre>\n", escape_xml(g)));
    }

    for person in &series.people {
        if let Some(ref t) = person.person_type {
            if t.eq_ignore_ascii_case("Actor") {
                out.push_str("  <actor>\n");
                out.push_str(&format!("    <name>{}</name>\n", escape_xml(&person.name)));
                if let Some(ref r) = person.role {
                    out.push_str(&format!("    <role>{}</role>\n", escape_xml(r)));
                }
                out.push_str("  </actor>\n");
            }
        }
    }

    out.push_str("</tvshow>\n");
    out
}

pub fn generate_episode_nfo(ep: &EpisodeItem, series_title: &str) -> String {
    let mut out = String::with_capacity(512);
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n");
    out.push_str("<episodedetails>\n");

    out.push_str(&format!("  <title>{}</title>\n", escape_xml(&ep.name)));
    out.push_str(&format!("  <showtitle>{}</showtitle>\n", escape_xml(series_title)));

    if let Some(s) = ep.parent_index_number {
        out.push_str(&format!("  <season>{}</season>\n", s));
    }
    if let Some(e) = ep.index_number {
        out.push_str(&format!("  <episode>{}</episode>\n", e));
    }

    if let Some(ref plot) = ep.overview {
        out.push_str(&format!("  <plot>{}</plot>\n", escape_xml(plot)));
    }

    if let Some(rating) = ep.community_rating {
        out.push_str(&format!("  <rating>{:.1}</rating>\n", rating));
    }

    if let Some(ref date) = ep.premiere_date {
        let clean = date.split('T').next().unwrap_or(date);
        out.push_str(&format!("  <aired>{}</aired>\n", escape_xml(clean)));
    }

    out.push_str("</episodedetails>\n");
    out
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn write_strm_if_changed(path: &Path, stream_url: &str) -> Result<bool> {
    if path.exists() {
        if let Ok(existing) = fs::read_to_string(path) {
            if existing.trim() == stream_url.trim() {
                return Ok(false); // Unchanged
            }
        }
    }
    fs::write(path, format!("{}\n", stream_url.trim()))?;
    Ok(true) // Created or updated
}

fn extract_real_basename(path_str: &str) -> Option<String> {
    let s = path_str.trim();
    if s.is_empty() {
        return None;
    }

    // Split on either Windows backslash or Linux slash
    let last_component = s.split(&['/', '\\'][..]).filter(|p| !p.is_empty()).last()?;

    // Strip video extensions
    let ext_list = [
        ".mkv", ".mp4", ".avi", ".ts", ".m2ts", ".webm", ".mov", ".wmv", ".iso", ".flv", ".vob",
        ".mpg", ".mpeg", ".m4v",
    ];

    let mut name = last_component;
    for ext in &ext_list {
        if name.to_lowercase().ends_with(ext) {
            name = &name[..name.len() - ext.len()];
            break;
        }
    }

    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

pub fn sanitize_folder(name: &str) -> String {
    let sanitized = sanitize_filename::sanitize_with_options(
        name,
        sanitize_filename::Options {
            windows: true,
            truncate: true,
            replacement: "_",
        },
    );
    if sanitized.is_empty() {
        "Unknown".to_string()
    } else {
        sanitized
    }
}

pub fn sanitize_file(name: &str) -> String {
    let sanitized = sanitize_filename::sanitize_with_options(
        name,
        sanitize_filename::Options {
            windows: true,
            truncate: true,
            replacement: "_",
        },
    );
    if sanitized.is_empty() {
        "file".to_string()
    } else {
        sanitized
    }
}

fn collect_strm_files(dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_strm_files(&path, files)?;
        } else if path.extension().and_then(|s| s.to_str()) == Some("strm") {
            files.push(path);
        }
    }
    Ok(())
}

fn clean_empty_and_orphaned_dirs(dir: &Path, root: &Path) {
    if !dir.is_dir() {
        return;
    }

    // 1. Recurse into child directories first
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                clean_empty_and_orphaned_dirs(&path, root);
            }
        }
    }

    if dir == root || dir == Path::new("/") {
        return;
    }

    // 2. Re-read directory contents after children may have been deleted
    if let Ok(entries) = fs::read_dir(dir) {
        let mut has_strm = false;
        let mut has_subdirs = false;

        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                has_subdirs = true;
            } else if path.extension().and_then(|s| s.to_str()) == Some("strm") {
                has_strm = true;
            }
        }

        // If a directory has no subdirectories and no .strm files, it is an orphaned or empty folder
        // Remove auxiliary files (nfo, images) and remove the directory
        if !has_subdirs && !has_strm {
            if let Ok(leftovers) = fs::read_dir(dir) {
                for item in leftovers.flatten() {
                    let _ = fs::remove_file(item.path());
                }
            }
            let _ = fs::remove_dir(dir);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_real_basename() {
        let win_path = r"A:\2 Hearts (2020) [imdbid-tt5616176]\2 Hearts (2020) [imdbid-tt5616176] - 2160P - Filmbol #164059.mkv";
        assert_eq!(
            extract_real_basename(win_path).unwrap(),
            "2 Hearts (2020) [imdbid-tt5616176] - 2160P - Filmbol #164059"
        );

        let nix_path = "/data/movies/Inception.2010.1080p.Remux.mkv";
        assert_eq!(
            extract_real_basename(nix_path).unwrap(),
            "Inception.2010.1080p.Remux"
        );
    }

    #[test]
    fn test_escape_xml() {
        assert_eq!(escape_xml("Tom & Jerry <movie> \"great\""), "Tom &amp; Jerry &lt;movie&gt; &quot;great&quot;");
    }

    #[test]
    fn test_generate_movie_nfo() {
        use std::collections::HashMap;
        let mut provider_ids = HashMap::new();
        provider_ids.insert("Imdb".to_string(), "tt0816692".to_string());
        provider_ids.insert("Tmdb".to_string(), "157336".to_string());

        let movie = MovieItem {
            id: "abc".to_string(),
            name: "Interstellar".to_string(),
            original_title: Some("Interstellar".to_string()),
            path: None,
            production_year: Some(2014),
            premiere_date: Some("2014-11-05T00:00:00Z".to_string()),
            overview: Some("A team of explorers...".to_string()),
            genres: vec!["Adventure".to_string(), "Sci-Fi".to_string()],
            community_rating: Some(8.7),
            official_rating: Some("PG-13".to_string()),
            run_time_ticks: Some(101400000000), // 169 mins
            taglines: vec!["Mankind was born on Earth...".to_string()],
            provider_ids,
            people: vec![
                crate::jellyfin::Person {
                    name: "Christopher Nolan".to_string(),
                    person_type: Some("Director".to_string()),
                    role: None,
                },
                crate::jellyfin::Person {
                    name: "Matthew McConaughey".to_string(),
                    person_type: Some("Actor".to_string()),
                    role: Some("Cooper".to_string()),
                },
            ],
            image_tags: HashMap::new(),
            backdrop_image_tags: vec![],
            media_sources: vec![],
        };

        let xml = generate_movie_nfo(&movie);
        assert!(xml.contains("<title>Interstellar</title>"));
        assert!(xml.contains("<year>2014</year>"));
        assert!(xml.contains("<runtime>169</runtime>"));
        assert!(xml.contains("<director>Christopher Nolan</director>"));
        assert!(xml.contains("<name>Matthew McConaughey</name>"));
        assert!(xml.contains("<uniqueid type=\"imdb\" default=\"true\">tt0816692</uniqueid>"));
    }

    #[test]
    fn test_generate_episode_nfo() {
        let ep = EpisodeItem {
            id: "ep1".to_string(),
            name: "Pilot".to_string(),
            series_id: Some("s1".to_string()),
            series_name: Some("Breaking Bad".to_string()),
            season_id: Some("season1".to_string()),
            season_name: Some("Season 1".to_string()),
            index_number: Some(1),
            parent_index_number: Some(1),
            path: None,
            overview: Some("Walter White starts cooking...".to_string()),
            community_rating: Some(9.2),
            premiere_date: Some("2008-01-20T00:00:00Z".to_string()),
            image_tags: HashMap::new(),
            media_sources: vec![],
        };

        let xml = generate_episode_nfo(&ep, "Breaking Bad (2008)");
        assert!(xml.contains("<title>Pilot</title>"));
        assert!(xml.contains("<showtitle>Breaking Bad (2008)</showtitle>"));
        assert!(xml.contains("<season>1</season>"));
        assert!(xml.contains("<episode>1</episode>"));
        assert!(xml.contains("<plot>Walter White starts cooking...</plot>"));
        assert!(xml.contains("<rating>9.2</rating>"));
        assert!(xml.contains("<aired>2008-01-20</aired>"));
    }

    #[test]
    fn test_clean_orphans_removes_nested_empty_directories() {
        let root = std::env::temp_dir().join(format!("test_strm_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir_all(&root).unwrap();

        // Simulate old folder without year: TV Shows/Breaking Bad/Season 01/ep1.strm
        let old_series = root.join("Breaking Bad");
        let old_season = old_series.join("Season 01");
        fs::create_dir_all(&old_season).unwrap();
        let orphan_strm = old_season.join("ep1.strm");
        fs::write(&orphan_strm, "http://example.com").unwrap();
        let orphan_nfo = old_season.join("ep1.nfo");
        fs::write(&orphan_nfo, "test nfo").unwrap();

        // Simulate active file in new folder: TV Shows/Breaking Bad (2008)/Season 01/ep1.strm
        let new_series = root.join("Breaking Bad (2008)");
        let new_season = new_series.join("Season 01");
        fs::create_dir_all(&new_season).unwrap();
        let active_strm = new_season.join("ep1.strm");
        fs::write(&active_strm, "http://example.com").unwrap();

        let mut active_files = HashSet::new();
        active_files.insert(active_strm.clone());

        let generator = StrmGenerator::new(
            "http://jellyfin".to_string(),
            "token".to_string(),
            root.clone(),
            true,
            true,
            true,
            4,
        );

        let deleted = generator.clean_orphans(&active_files).unwrap();
        assert_eq!(deleted, 1);

        // Active file and its folder must still exist
        assert!(active_strm.exists());
        assert!(new_series.exists());

        // Orphan file and its nested directories must be completely removed
        assert!(!orphan_strm.exists());
        assert!(!orphan_nfo.exists());
        assert!(!old_season.exists());
        assert!(!old_series.exists());

        let _ = fs::remove_dir_all(&root);
    }
}
