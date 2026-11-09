use crate::jellyfin::{EpisodeItem, MovieItem};
use anyhow::Result;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

pub struct StrmGenerator {
    base_url: String,
    token: String,
    output_dir: PathBuf,
    clean_deleted: bool,
}

impl StrmGenerator {
    pub fn new(base_url: String, token: String, output_dir: PathBuf, clean_deleted: bool) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            token,
            output_dir,
            clean_deleted,
        }
    }

    pub fn target_library_dir(&self, library_name: &str) -> PathBuf {
        self.output_dir.join(sanitize_folder(library_name))
    }

    pub fn generate_movie_batch(
        &self,
        movies: &[MovieItem],
        target_dir: &Path,
        active_files: &mut HashSet<PathBuf>,
    ) -> Result<(usize, usize)> {
        let mut created_count = 0;
        let mut unchanged_count = 0;

        for movie in movies {
            let folder_name = if let Some(year) = movie.production_year {
                if year > 0 {
                    format!("{} ({})", movie.name, year)
                } else {
                    movie.name.clone()
                }
            } else {
                movie.name.clone()
            };

            let movie_dir = target_dir.join(sanitize_folder(&folder_name));
            if let Err(e) = fs::create_dir_all(&movie_dir) {
                warn!("Failed to create folder {:?}: {}", movie_dir, e);
                continue;
            }

            if movie.media_sources.is_empty() {
                let file_name = format!("{}.strm", sanitize_file(&movie.name));
                let file_path = movie_dir.join(file_name);
                let stream_url = format!(
                    "{}/Videos/{}/stream?static=true&api_key={}",
                    self.base_url, movie.id, self.token
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
                            if movie.media_sources.len() > 1 {
                                let label = ms.name.as_deref().unwrap_or("version");
                                format!("{} - {}", movie.name, label)
                            } else {
                                movie.name.clone()
                            }
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

        Ok((created_count, unchanged_count))
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
                    if let Some(idx) = ep.index_number {
                        format!("{} - E{:02} - {}", series_folder, idx, ep.name)
                    } else {
                        format!("{} - {}", series_folder, ep.name)
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
                            if let Some(idx) = ep.index_number {
                                format!("{} - E{:02} - {}", series_folder, idx, ep.name)
                            } else {
                                format!("{} - {}", series_folder, ep.name)
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

        // Clean up empty directories
        remove_empty_dirs(&self.output_dir);

        if deleted_count > 0 {
            info!("Cleaned up {} removed strm files from disk", deleted_count);
        }

        Ok(deleted_count)
    }
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

fn sanitize_folder(name: &str) -> String {
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

fn sanitize_file(name: &str) -> String {
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

fn remove_empty_dirs(dir: &Path) {
    if !dir.is_dir() {
        return;
    }
    if let Ok(entries) = fs::read_dir(dir) {
        let mut is_empty = true;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                remove_empty_dirs(&path);
                if path.is_dir() {
                    // Check if directory became empty after recursive cleanup
                    if let Ok(mut sub_entries) = fs::read_dir(&path) {
                        if sub_entries.next().is_none() {
                            let _ = fs::remove_dir(&path);
                        } else {
                            is_empty = false;
                        }
                    }
                }
            } else {
                is_empty = false;
            }
        }
        if is_empty && dir != Path::new("/") {
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
}
