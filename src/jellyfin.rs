use anyhow::{bail, Context, Result};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde::Deserialize;
use std::collections::HashMap;
use tracing::info;

#[derive(Debug, Clone, Deserialize)]
pub struct Library {
    #[serde(rename = "Id")]
    pub id: String,
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "CollectionType")]
    pub collection_type: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MediaSource {
    #[serde(rename = "Id")]
    pub id: String,
    #[serde(rename = "Name")]
    pub name: Option<String>,
    #[serde(rename = "Path")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Person {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Type")]
    pub person_type: Option<String>,
    #[serde(rename = "Role")]
    pub role: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MovieItem {
    #[serde(rename = "Id")]
    pub id: String,
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "OriginalTitle")]
    pub original_title: Option<String>,
    #[serde(rename = "Path")]
    pub path: Option<String>,
    #[serde(rename = "ProductionYear")]
    pub production_year: Option<u32>,
    #[serde(rename = "PremiereDate")]
    pub premiere_date: Option<String>,
    #[serde(rename = "Overview")]
    pub overview: Option<String>,
    #[serde(rename = "Genres", default)]
    pub genres: Vec<String>,
    #[serde(rename = "CommunityRating")]
    pub community_rating: Option<f32>,
    #[serde(rename = "OfficialRating")]
    pub official_rating: Option<String>,
    #[serde(rename = "RunTimeTicks")]
    pub run_time_ticks: Option<i64>,
    #[serde(rename = "Taglines", default)]
    pub taglines: Vec<String>,
    #[serde(rename = "ProviderIds", default)]
    pub provider_ids: HashMap<String, String>,
    #[serde(rename = "People", default)]
    pub people: Vec<Person>,
    #[serde(rename = "ImageTags", default)]
    pub image_tags: HashMap<String, String>,
    #[serde(rename = "BackdropImageTags", default)]
    pub backdrop_image_tags: Vec<String>,
    #[serde(rename = "MediaSources", default)]
    pub media_sources: Vec<MediaSource>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SeriesItem {
    #[serde(rename = "Id")]
    pub id: String,
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "OriginalTitle")]
    pub original_title: Option<String>,
    #[serde(rename = "ProductionYear")]
    pub production_year: Option<u32>,
    #[serde(rename = "PremiereDate")]
    pub premiere_date: Option<String>,
    #[serde(rename = "Overview")]
    pub overview: Option<String>,
    #[serde(rename = "Genres", default)]
    pub genres: Vec<String>,
    #[serde(rename = "CommunityRating")]
    pub community_rating: Option<f32>,
    #[serde(rename = "OfficialRating")]
    pub official_rating: Option<String>,
    #[serde(rename = "Taglines", default)]
    pub taglines: Vec<String>,
    #[serde(rename = "ProviderIds", default)]
    pub provider_ids: HashMap<String, String>,
    #[serde(rename = "People", default)]
    pub people: Vec<Person>,
    #[serde(rename = "ImageTags", default)]
    pub image_tags: HashMap<String, String>,
    #[serde(rename = "BackdropImageTags", default)]
    pub backdrop_image_tags: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EpisodeItem {
    #[serde(rename = "Id")]
    pub id: String,
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "SeriesId")]
    pub series_id: Option<String>,
    #[serde(rename = "SeriesName")]
    pub series_name: Option<String>,
    #[allow(dead_code)]
    #[serde(rename = "SeasonId")]
    pub season_id: Option<String>,
    #[serde(rename = "SeasonName")]
    pub season_name: Option<String>,
    #[serde(rename = "IndexNumber")]
    pub index_number: Option<u32>,
    #[serde(rename = "ParentIndexNumber")]
    pub parent_index_number: Option<u32>,
    #[serde(rename = "Path")]
    pub path: Option<String>,
    #[serde(rename = "Overview")]
    pub overview: Option<String>,
    #[serde(rename = "CommunityRating")]
    pub community_rating: Option<f32>,
    #[serde(rename = "PremiereDate")]
    pub premiere_date: Option<String>,
    #[serde(rename = "ImageTags", default)]
    pub image_tags: HashMap<String, String>,
    #[serde(rename = "MediaSources", default)]
    pub media_sources: Vec<MediaSource>,
}

#[derive(Deserialize)]
struct AuthResponse {
    #[serde(rename = "AccessToken")]
    access_token: String,
    #[serde(rename = "User")]
    user: UserInfo,
}

#[derive(Deserialize)]
struct UserInfo {
    #[serde(rename = "Id")]
    id: String,
}

#[derive(Deserialize)]
struct ItemsResponse<T> {
    #[serde(rename = "Items")]
    items: Vec<T>,
    #[serde(rename = "TotalRecordCount")]
    total_record_count: Option<usize>,
}

pub struct JellyfinClient {
    client: reqwest::Client,
    pub base_url: String,
    username: Option<String>,
    password: Option<String>,
    pub user_id: Option<String>,
    pub token: Option<String>,
    device_id: String,
}

impl JellyfinClient {
    pub fn new(
        base_url: String,
        username: Option<String>,
        password: Option<String>,
        api_key: Option<String>,
    ) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .unwrap_or_default();

        let device_id = "jellyfin-strm-nas".to_string();

        Self {
            client,
            base_url,
            username,
            password,
            user_id: None,
            token: api_key,
            device_id,
        }
    }

    fn auth_header(&self) -> String {
        let token_part = if let Some(ref t) = self.token {
            format!(", Token=\"{}\"", t)
        } else {
            String::new()
        };
        format!(
            "MediaBrowser Client=\"JellyfinSTRM\", Device=\"NAS\", DeviceId=\"{}\", Version=\"1.0.0\"{}",
            self.device_id, token_part
        )
    }

    pub async fn authenticate(&mut self) -> Result<()> {
        if let (Some(u), Some(p)) = (&self.username, &self.password) {
            info!("Authenticating with Jellyfin as user '{}'...", u);
            let url = format!("{}/Users/AuthenticateByName", self.base_url);

            let mut headers = HeaderMap::new();
            headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&self.auth_header())?,
            );

            let body = serde_json::json!({
                "Username": u,
                "Pw": p,
            });

            let resp = self
                .client
                .post(&url)
                .headers(headers)
                .json(&body)
                .send()
                .await
                .context("Failed to send login request to Jellyfin")?;

            if !resp.status().is_success() {
                let status = resp.status();
                let txt = resp.text().await.unwrap_or_default();
                bail!("Jellyfin authentication failed (HTTP {}): {}", status, txt);
            }

            let auth_data: AuthResponse = resp
                .json()
                .await
                .context("Failed to parse Jellyfin authentication response")?;

            self.token = Some(auth_data.access_token);
            self.user_id = Some(auth_data.user.id);
            info!("Successfully authenticated! User ID: {}", self.user_id.as_deref().unwrap_or(""));
            Ok(())
        } else if self.token.is_some() {
            // Static API key provided, resolve User ID
            let url = format!("{}/Users", self.base_url);
            let mut headers = HeaderMap::new();
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&self.auth_header())?,
            );
            let resp = self.client.get(&url).headers(headers).send().await?;
            if resp.status().is_success() {
                let users: Vec<UserInfo> = resp.json().await.unwrap_or_default();
                if let Some(first) = users.first() {
                    self.user_id = Some(first.id.clone());
                    info!("Resolved User ID from API key: {}", first.id);
                }
            }
            Ok(())
        } else {
            bail!("No username/password or API key provided for Jellyfin");
        }
    }

    pub async fn get_libraries(&self) -> Result<Vec<Library>> {
        let user_id = self.user_id.as_ref().context("User ID not resolved")?;
        let url = format!("{}/Users/{}/Views", self.base_url, user_id);

        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&self.auth_header())?,
        );

        let resp = self
            .client
            .get(&url)
            .headers(headers)
            .send()
            .await
            .context("Failed to fetch libraries from Jellyfin")?;

        if resp.status().as_u16() == 401 {
            bail!("Unauthorized (token expired)");
        }

        let data: ItemsResponse<Library> = resp.json().await?;
        Ok(data.items)
    }

    async fn send_with_retry(&self, req_builder: reqwest::RequestBuilder) -> Result<reqwest::Response> {
        let mut attempts = 0;
        loop {
            attempts += 1;
            let req = req_builder
                .try_clone()
                .ok_or_else(|| anyhow::anyhow!("Request not cloneable"))?;
            match req.send().await {
                Ok(resp) => return Ok(resp),
                Err(e) if attempts < 3 => {
                    tracing::warn!(
                        "Jellyfin request failed (attempt {}/3): {}. Retrying in 2s...",
                        attempts,
                        e
                    );
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    pub async fn get_movies_page(
        &self,
        library_id: &str,
        start_index: usize,
        limit: usize,
    ) -> Result<(Vec<MovieItem>, usize)> {
        let user_id = self.user_id.as_ref().context("User ID not resolved")?;
        let url = format!("{}/Users/{}/Items", self.base_url, user_id);
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&self.auth_header())?,
        );

        let query = [
            ("ParentId", library_id),
            ("IncludeItemTypes", "Movie"),
            ("Recursive", "true"),
            (
                "Fields",
                "MediaSources,Path,OriginalTitle,ProductionYear,PremiereDate,Overview,Genres,CommunityRating,OfficialRating,RunTimeTicks,Taglines,ProviderIds,People,ImageTags,BackdropImageTags",
            ),
            ("SortBy", "SortName"),
            ("SortOrder", "Ascending"),
            ("StartIndex", &start_index.to_string()),
            ("Limit", &limit.to_string()),
        ];

        let req = self.client.get(&url).headers(headers).query(&query);
        let resp = self
            .send_with_retry(req)
            .await
            .context("Failed to fetch movies from Jellyfin")?;

        if resp.status().as_u16() == 401 {
            bail!("Unauthorized (token expired)");
        }

        let data: ItemsResponse<MovieItem> = resp.json().await?;
        let total = data.total_record_count.unwrap_or(data.items.len());
        Ok((data.items, total))
    }

    pub async fn get_series_page(
        &self,
        library_id: &str,
        start_index: usize,
        limit: usize,
    ) -> Result<(Vec<SeriesItem>, usize)> {
        let user_id = self.user_id.as_ref().context("User ID not resolved")?;
        let url = format!("{}/Users/{}/Items", self.base_url, user_id);
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&self.auth_header())?,
        );

        let query = [
            ("ParentId", library_id),
            ("IncludeItemTypes", "Series"),
            ("Recursive", "true"),
            (
                "Fields",
                "OriginalTitle,ProductionYear,PremiereDate,Overview,Genres,CommunityRating,OfficialRating,Taglines,ProviderIds,People,ImageTags,BackdropImageTags",
            ),
            ("SortBy", "SortName"),
            ("SortOrder", "Ascending"),
            ("StartIndex", &start_index.to_string()),
            ("Limit", &limit.to_string()),
        ];

        let req = self.client.get(&url).headers(headers).query(&query);
        let resp = self
            .send_with_retry(req)
            .await
            .context("Failed to fetch series from Jellyfin")?;

        if resp.status().as_u16() == 401 {
            bail!("Unauthorized (token expired)");
        }

        let data: ItemsResponse<SeriesItem> = resp.json().await?;
        let total = data.total_record_count.unwrap_or(data.items.len());
        Ok((data.items, total))
    }

    pub async fn get_episodes_page(
        &self,
        library_id: &str,
        start_index: usize,
        limit: usize,
    ) -> Result<(Vec<EpisodeItem>, usize)> {
        let user_id = self.user_id.as_ref().context("User ID not resolved")?;
        let url = format!("{}/Users/{}/Items", self.base_url, user_id);
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&self.auth_header())?,
        );

        let query = [
            ("ParentId", library_id),
            ("IncludeItemTypes", "Episode"),
            ("Recursive", "true"),
            (
                "Fields",
                "MediaSources,Path,SeriesId,SeriesName,SeasonId,SeasonName,IndexNumber,ParentIndexNumber,Overview,CommunityRating,PremiereDate,ImageTags",
            ),
            ("SortBy", "SortName"),
            ("SortOrder", "Ascending"),
            ("StartIndex", &start_index.to_string()),
            ("Limit", &limit.to_string()),
        ];

        let req = self.client.get(&url).headers(headers).query(&query);
        let resp = self
            .send_with_retry(req)
            .await
            .context("Failed to fetch episodes from Jellyfin")?;

        if resp.status().as_u16() == 401 {
            bail!("Unauthorized (token expired)");
        }

        let data: ItemsResponse<EpisodeItem> = resp.json().await?;
        let total = data.total_record_count.unwrap_or(data.items.len());
        Ok((data.items, total))
    }

    pub async fn download_image(&self, item_id: &str, image_type: &str) -> Result<Option<Vec<u8>>> {
        let path = match image_type {
            "Primary" => format!("/Items/{}/Images/Primary", item_id),
            "Backdrop" => format!("/Items/{}/Images/Backdrop/0", item_id),
            "Logo" => format!("/Items/{}/Images/Logo", item_id),
            other => format!("/Items/{}/Images/{}", item_id, other),
        };

        let url = format!("{}{}", self.base_url, path);
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&self.auth_header())?,
        );

        let req = self.client.get(&url).headers(headers);
        let resp = self.send_with_retry(req).await?;

        if resp.status().is_success() {
            let bytes = resp.bytes().await?;
            Ok(Some(bytes.to_vec()))
        } else if resp.status().as_u16() == 404 {
            Ok(None)
        } else {
            bail!("Failed to download {} for item {}: HTTP {}", image_type, item_id, resp.status());
        }
    }
}
