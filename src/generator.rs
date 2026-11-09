use crate::jellyfin::{EpisodeItem, JellyfinClient, MovieItem, SeriesItem};
use anyhow::Result;
use std::collections::HashSet;
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
            }

            // 2. Queue images if enabled (only if file doesn't already exist on disk)
            if self.download_images {
                let poster_path = movie_dir.join("poster.jpg");
                if !poster_path.exists() && movie.image_tags.contains_key("Primary") {
                    image_tasks.push((movie.id.clone(), "Primary".to_string(), poster_path));
                }

                let bg_path = movie_dir.join("background.jpg");
                if !bg_path.exists() && !movie.backdrop_image_tags.is_empty() {
                    image_tasks.push((movie.id.clone(), "Backdrop".to_string(), bg_path));
                }

                let logo_path = movie_dir.join("logo.png");
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
    ) -> Result<()> {
        let mut image_tasks: Vec<(String, String, PathBuf)> = Vec::new();

        for series in series_items {
            let folder_name = Self::generate_movie_folder_name(&series.name, series.production_year);
            let series_dir = target_dir.join(sanitize_folder(&folder_name));

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
            }

            // Series images
            if self.download_images {
                let poster_path = series_dir.join("poster.jpg");
                if !poster_path.exists() && series.image_tags.contains_key("Primary") {
                    image_tasks.push((series.id.clone(), "Primary".to_string(), poster_path));
                }

                let bg_path = series_dir.join("background.jpg");
                if !bg_path.exists() && !series.backdrop_image_tags.is_empty() {
                    image_tasks.push((series.id.clone(), "Backdrop".to_string(), bg_path));
                }

                let logo_path = series_dir.join("logo.png");
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

    pub fn generate_episode_batch(
        &self,
        episodes: &[EpisodeItem],
        target_dir: &Path,
        active_files: &mut HashSet<PathBuf>,
    ) -> Result<(usize, usize)> {
        let mut created_count = 0;
        let mut unchanged_count = 0;

        for ep in episodes {
            let series_folder = ep.series_name.as_deref().unwrap_or("Unknown Series");
            let season_folder = ep.season_name.as_deref().unwrap_or("Season 1");

            let episode_dir = target_dir
                .join(sanitize_folder(series_folder))
                .join(sanitize_folder(season_folder));

            if let Err(e) = fs::create_dir_all(&episode_dir) {
                warn!("Failed to create folder {:?}: {}", episode_dir, e);
                continue;
            }

            if ep.media_sources.is_empty() {
                let base_name = ep.path.as_deref().and_then(extract_real_basename).unwrap_or_else(|| {
                    match (ep.parent_index_number, ep.index_number) {
                        (Some(s), Some(e)) => format!("{} - S{:02}E{:02} - {}", series_folder, s, e, ep.name),
                        (_, Some(e)) => format!("{} - E{:02} - {}", series_folder, e, ep.name),
                        _ => format!("{} - {}", series_folder, ep.name),
                    }
                });

                let file_name = format!("{}.strm", sanitize_file(&base_name));
                let file_path = episode_dir.join(file_name);
                let stream_url = format!(
                    "{}/Videos/{}/stream?static=true&api_key={}",
                    self.base_url, ep.id, self.token
                );

                if write_strm_if_changed(&file_path, &stream_url)? {
                    created_count += 1;
                } else {
                    unchanged_count += 1;
                }
                active_files.insert(file_path);
            } else {
                for ms in &ep.media_sources {
                    let base_name = ms
                        .path
                        .as_deref()
                        .or(ep.path.as_deref())
                        .and_then(extract_real_basename)
                        .unwrap_or_else(|| {
                            match (ep.parent_index_number, ep.index_number) {
                                (Some(s), Some(e)) => format!("{} - S{:02}E{:02} - {}", series_folder, s, e, ep.name),
                                (_, Some(e)) => format!("{} - E{:02} - {}", series_folder, e, ep.name),
                                _ => format!("{} - {}", series_folder, ep.name),
                            }
                        });

                    let file_name = format!("{}.strm", sanitize_file(&base_name));
                    let file_path = episode_dir.join(file_name);
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
                }
            }
        }

        // Clean up empty directories and leftover metadata for deleted movies
        clean_empty_and_orphaned_dirs(&self.output_dir);

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

fn clean_empty_and_orphaned_dirs(dir: &Path) {
    if !dir.is_dir() {
        return;
    }

    if let Ok(entries) = fs::read_dir(dir) {
        let mut has_strm = false;
        let mut has_subdirs = false;

        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                has_subdirs = true;
                clean_empty_and_orphaned_dirs(&path);
            } else if path.extension().and_then(|s| s.to_str()) == Some("strm") {
                has_strm = true;
            }
        }

        // If a directory has no subdirectories and no .strm files, it is an orphaned or empty folder
        // Remove auxiliary files (nfo, images) and remove the directory
        if !has_subdirs && !has_strm && dir != Path::new("/") {
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
}
