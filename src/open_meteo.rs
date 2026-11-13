use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};

const DEFAULT_GEOCODING_URL: &str = "https://geocoding-api.open-meteo.com/v1/search";
const DEFAULT_FORECAST_URL: &str = "https://api.open-meteo.com/v1/forecast";
const DEFAULT_AIR_QUALITY_URL: &str = "https://air-quality-api.open-meteo.com/v1/air-quality";

#[derive(Debug, Deserialize)]
struct GeoResponse {
    results: Option<Vec<GeoLocation>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GeoLocation {
    pub name: String,
    pub country: Option<String>,
    pub latitude: f64,
    pub longitude: f64,
    pub timezone: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeatherReport {
    pub location: GeoLocation,
    pub current: serde_json::Value,
    pub hourly: serde_json::Value,
    pub daily: serde_json::Value,
    pub air_quality: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct OpenMeteoClient {
    http: reqwest::Client,
    geocoding_url: String,
    forecast_url: String,
    air_quality_url: String,
}

impl Default for OpenMeteoClient {
    fn default() -> Self {
        Self::new()
    }
}

impl OpenMeteoClient {
    pub fn new() -> Self {
        Self::with_endpoints(
            DEFAULT_GEOCODING_URL,
            DEFAULT_FORECAST_URL,
            DEFAULT_AIR_QUALITY_URL,
        )
    }

    pub fn with_endpoints(
        geocoding_url: impl Into<String>,
        forecast_url: impl Into<String>,
        air_quality_url: impl Into<String>,
    ) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(concat!("meteo-mcp-rs/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            http,
            geocoding_url: geocoding_url.into(),
            forecast_url: forecast_url.into(),
            air_quality_url: air_quality_url.into(),
        }
    }

    pub async fn geocode(&self, city: &str) -> Result<GeoLocation> {
        let trimmed = city.trim();
        if trimmed.is_empty() {
            return Err(anyhow!("City name must not be empty"));
        }

        let resp: GeoResponse = self
            .http
            .get(&self.geocoding_url)
            .query(&[
                ("name", trimmed),
                ("count", "1"),
                ("language", "pl"),
                ("format", "json"),
            ])
            .send()
            .await
            .context("Failed to connect to the Open-Meteo Geocoding API")?
            .error_for_status()
            .context("Open-Meteo Geocoding API returned an HTTP error")?
            .json()
            .await
            .context("Invalid JSON from the Open-Meteo Geocoding API")?;

        resp.results
            .and_then(|mut r| r.drain(..).next())
            .ok_or_else(|| anyhow!("Location not found: {trimmed}"))
    }

    pub async fn get_full_report(&self, city: &str) -> Result<WeatherReport> {
        let loc = self.geocode(city).await?;
        self.get_report_by_coords(loc).await
    }

    pub async fn get_report_by_coords(&self, loc: GeoLocation) -> Result<WeatherReport> {
        let lat = loc.latitude.to_string();
        let lon = loc.longitude.to_string();

        let weather_req = self
            .http
            .get(&self.forecast_url)
            .query(&[
                ("latitude", lat.as_str()),
                ("longitude", lon.as_str()),
                (
                    "current",
                    "temperature_2m,relative_humidity_2m,apparent_temperature,weather_code,wind_speed_10m,surface_pressure,precipitation",
                ),
                (
                    "hourly",
                    "temperature_2m,precipitation_probability,weather_code",
                ),
                (
                    "daily",
                    "weather_code,temperature_2m_max,temperature_2m_min,precipitation_sum,uv_index_max",
                ),
                ("timezone", "auto"),
                ("forecast_days", "7"),
            ])
            .send();

        let aqi_req = self
            .http
            .get(&self.air_quality_url)
            .query(&[
                ("latitude", lat.as_str()),
                ("longitude", lon.as_str()),
                ("current", "european_aqi,pm10,pm2_5"),
            ])
            .send();

        let (weather_res, aqi_res) = tokio::join!(weather_req, aqi_req);

        let weather_json: serde_json::Value = weather_res
            .context("Failed to connect to the Open-Meteo Forecast API")?
            .error_for_status()
            .context("Open-Meteo Forecast API returned an HTTP error")?
            .json()
            .await
            .context("Invalid JSON from the Open-Meteo Forecast API")?;

        // Air quality API is non-critical in edge coordinates (e.g. oceans); degrade gracefully.
        let air_quality = match aqi_res {
            Ok(resp) if resp.status().is_success() => resp
                .json::<serde_json::Value>()
                .await
                .map(|v| v["current"].clone())
                .unwrap_or(serde_json::Value::Null),
            _ => serde_json::Value::Null,
        };

        Ok(WeatherReport {
            location: loc,
            current: weather_json["current"].clone(),
            hourly: weather_json["hourly"].clone(),
            daily: weather_json["daily"].clone(),
            air_quality,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn spawn_mock_server<F>(handler: F) -> String
    where
        F: Fn(&str) -> (&'static str, String) + Send + Sync + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handler = std::sync::Arc::new(handler);

        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let handler = handler.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let n = socket.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let first_line = req.lines().next().unwrap_or("");
                    let path = first_line.split_whitespace().nth(1).unwrap_or("/");
                    let (status, body) = handler(path);
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });

        format!("http://{addr}")
    }

    #[tokio::test]
    async fn full_report_fetches_weather_and_air_quality() {
        let base = spawn_mock_server(|path| {
            if path.starts_with("/geo") {
                (
                    "200 OK",
                    json!({
                        "results": [{
                            "name": "Warsaw",
                            "country": "Poland",
                            "latitude": 52.2297,
                            "longitude": 21.0122,
                            "timezone": "Europe/Warsaw"
                        }]
                    })
                    .to_string(),
                )
            } else if path.starts_with("/forecast") {
                (
                    "200 OK",
                    json!({
                        "current": {
                            "temperature_2m": 18.5,
                            "relative_humidity_2m": 60,
                            "apparent_temperature": 17.9,
                            "weather_code": 1,
                            "wind_speed_10m": 12.3,
                            "surface_pressure": 1015.0,
                            "precipitation": 0.0
                        },
                        "hourly": {
                            "time": ["2026-08-18T12:00", "2026-08-18T13:00"],
                            "temperature_2m": [18.5, 19.2],
                            "precipitation_probability": [10, 15],
                            "weather_code": [1, 2]
                        },
                        "daily": {
                            "time": ["2026-08-18"],
                            "weather_code": [1],
                            "temperature_2m_max": [22.0],
                            "temperature_2m_min": [13.0],
                            "precipitation_sum": [0.2]
                        }
                    })
                    .to_string(),
                )
            } else {
                (
                    "200 OK",
                    json!({
                        "current": {
                            "european_aqi": 18,
                            "pm2_5": 6.4,
                            "pm10": 11.2
                        }
                    })
                    .to_string(),
                )
            }
        })
        .await;

        let client = OpenMeteoClient::with_endpoints(
            format!("{base}/geo"),
            format!("{base}/forecast"),
            format!("{base}/aqi"),
        );

        let report = client.get_full_report("Warsaw").await.unwrap();
        assert_eq!(report.location.name, "Warsaw");
        assert_eq!(report.location.country.as_deref(), Some("Poland"));
        assert_eq!(report.current["temperature_2m"], 18.5);
        assert_eq!(report.air_quality["european_aqi"], 18);
    }

    #[tokio::test]
    async fn missing_city_returns_error_and_aqi_failure_degrades_gracefully() {
        let base = spawn_mock_server(|path| {
            if path.starts_with("/geo") {
                ("200 OK", json!({ "results": [] }).to_string())
            } else if path.starts_with("/forecast") {
                (
                    "200 OK",
                    json!({
                        "current": { "temperature_2m": 10.0 },
                        "hourly": { "time": [] },
                        "daily": { "time": [] }
                    })
                    .to_string(),
                )
            } else {
                (
                    "503 Service Unavailable",
                    json!({ "error": true }).to_string(),
                )
            }
        })
        .await;

        let client = OpenMeteoClient::with_endpoints(
            format!("{base}/geo"),
            format!("{base}/forecast"),
            format!("{base}/aqi"),
        );

        let err = client.geocode("AtlantisNowhere").await.unwrap_err();
        assert!(err.to_string().contains("AtlantisNowhere"));

        let report = client
            .get_report_by_coords(GeoLocation {
                name: "Atlantic".into(),
                country: None,
                latitude: 0.0,
                longitude: -30.0,
                timezone: None,
            })
            .await
            .unwrap();
        assert_eq!(report.current["temperature_2m"], 10.0);
        assert!(report.air_quality.is_null());
    }
}
