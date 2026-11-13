# Specyfikacja Techniczna i Implementacja: `meteo-mcp-rs` (Rust + MCP Apps)

Kompletna specyfikacja techniczna oraz szkielet produkcyjnego serwera MCP w języku **Rust** (`rmcp` + `tokio` + `reqwest`) zintegrowanego ze specyfikacją **MCP Apps** (`ui://` resource + JSON-RPC 2.0 `postMessage` bridge) dla publicznego API **Open-Meteo**.

Wersja natywna w Rust kompiluje się do pojedynczej binarki (~4–6 MB z wbudowanym interfejsem HTML/JS poprzez `include_str!`), uruchamia się w kilka milisekund przez `stdio` i nie wymaga żadnego środowiska uruchomieniowego po stronie użytkownika końcowego.

---

## 1. Specyfikacja Architektury (`meteo-mcp-rs`)

### Przepływ danych i warstwy
1. **Transport MCP:** `stdio` (JSON-RPC 2.0 po standardowym wejściu/wyjściu) przy użyciu oficjalnego crate’a `rmcp`.
2. **Źródła danych (bez kluczy API):**
   * **Geocoding API:** `https://geocoding-api.open-meteo.com/v1/search` — zamiana nazwy miasta na współrzędne geograficzne i strefę czasową.
   * **Weather Forecast API:** `https://api.open-meteo.com/v1/forecast` — prognoza 7-dniowa (`daily`), godzinowa (`hourly`) oraz warunki bieżące (`current`).
   * **Air Quality API:** `https://air-quality-api.open-meteo.com/v1/air-quality` — indeks jakości powietrza (EU AQI), pyły zawieszone PM2.5 oraz PM10.
3. **Warstwa MCP App (`ui://meteo/dashboard`):**
   * Serwer rejestruje zasób z typem MIME `text/html;profile=mcp-app`.
   * Narzędzia zwracające dane pogodowe zawierają w polu `_meta` referencję `"ui": { "resourceUri": "ui://meteo/dashboard" }`, co sygnalizuje hostowi MCP wyrenderowanie interaktywnego widżetu w izolowanym sandboxie `<iframe>`.
   * Kod UI jest pojedynczym, samowystarczalnym plikiem HTML z osadzonym CSS i czystym JS (Vanilla JS + SVG do wykresu), wkompilowanym w binarkę Rusta w czasie kompilacji (`include_str!("../ui/dashboard.html")`).

### Kontrakt Narzędzi (Tools) i Zasobów (Resources)

| Typ          | Nazwa / URI             | Widoczność         | Opis i rola w showcase                                       |
| :----------- | :---------------------- | :----------------- | :----------------------------------------------------------- |
| **Resource** | `ui://meteo/dashboard`  | Host UI            | Zwraca kod HTML/JS aplikacji MCP App (`text/html;profile=mcp-app`). |
| **Tool**     | `get_weather_dashboard` | `["model", "app"]` | Główne narzędzie wywoływane przez LLM (np. *"Jaka jest pogoda w Warszawie?"*). Zwraca podsumowanie tekstowe dla modelu oraz pełny obiekt JSON + powiązanie z `ui://meteo/dashboard`. |
| **Tool**     | `refresh_by_coords`     | `["app"]`          | Narzędzie wywoływane bezpośrednio z poziomu kliknięcia w UI (np. zmiana miasta lub odświeżenie danych) bez angażowania LLM. |

---

## 2. Struktura Projektu

```text
meteo-mcp-rs/
├── Cargo.toml
├── src/
│   ├── main.rs          # Punkt wejścia, inicjalizacja transportu stdio
│   ├── open_meteo.rs    # Klient HTTP i modele odpowiedzi Open-Meteo
│   └── server.rs        # Implementacja ServerHandler (Tools + MCP App Resources)
└── ui/
    └── dashboard.html   # Interaktywny komponent MCP App (wykres SVG + karta AQI)
```

---

## 3. Implementacja Kodu

### `Cargo.toml`

```toml
[package]
name = "meteo-mcp-rs"
version = "0.1.0"
edition = "2024"

[dependencies]
rmcp = { version = "0.1", features = ["server", "transport-io"] }
tokio = { version = "1", features = ["full"] }
reqwest = { version = "0.12", features = ["json", "rustls-tls"], default-features = false }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
schemars = "0.8"
anyhow = "1"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
```

---

### `src/open_meteo.rs` (Klient API Open-Meteo)

```rust
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
struct GeoResponse {
    results: Option<Vec<GeoLocation>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

pub struct OpenMeteoClient {
    http: reqwest::Client,
}

impl OpenMeteoClient {
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::new(),
        }
    }

    pub async fn geocode(&self, city: &str) -> Result<GeoLocation> {
        let clean_city = city.trim().replace(' ', "%20");
        let url = format!(
            "https://geocoding-api.open-meteo.com/v1/search?name={clean_city}&count=1&language=pl&format=json"
        );

        let resp: GeoResponse = self.http.get(&url).send().await?.json().await?;
        resp.results
            .and_then(|mut r| r.pop())
            .ok_or_else(|| anyhow!("Nie znaleziono lokalizacji: {city}"))
    }

    pub async fn get_full_report(&self, city: &str) -> Result<WeatherReport> {
        let loc = self.geocode(city).await?;
        self.get_report_by_coords(loc).await
    }

    pub async fn get_report_by_coords(&self, loc: GeoLocation) -> Result<WeatherReport> {
        let weather_url = format!(
            "https://api.open-meteo.com/v1/forecast?latitude={}&longitude={}&current=temperature_2m,relative_humidity_2m,apparent_temperature,weather_code,wind_speed_10m,surface_pressure&hourly=temperature_2m,precipitation_probability,weather_code&daily=weather_code,temperature_2m_max,temperature_2m_min,precipitation_sum,uv_index_max&timezone=auto&forecast_days=7",
            loc.latitude, loc.longitude
        );

        let aqi_url = format!(
            "https://air-quality-api.open-meteo.com/v1/air-quality?latitude={}&longitude={}&current=european_aqi,pm10,pm2_5",
            loc.latitude, loc.longitude
        );

        let (weather_res, aqi_res) = tokio::join!(
            self.http.get(&weather_url).send(),
            self.http.get(&aqi_url).send()
        );

        let weather_json: serde_json::Value = weather_res?.json().await?;
        let aqi_json: serde_json::Value = aqi_res?.json().await?;

        Ok(WeatherReport {
            location: loc,
            current: weather_json["current"].clone(),
            hourly: weather_json["hourly"].clone(),
            daily: weather_json["daily"].clone(),
            air_quality: aqi_json["current"].clone(),
        })
    }
}
```

---

### `src/server.rs` (Serwer MCP + Zasób MCP App)

```rust
use crate::open_meteo::{GeoLocation, OpenMeteoClient};
use rmcp::{
    Error as McpError, RoleServer, ServerHandler,
    model::*,
    service::RequestContext,
    tool,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

const DASHBOARD_HTML: &str = include_str!("../ui/dashboard.html");
const UI_RESOURCE_URI: &str = "ui://meteo/dashboard";

#[derive(Clone)]
pub struct MeteoServer {
    client: Arc<OpenMeteoClient>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WeatherQuery {
    #[schemars(description = "Nazwa miasta, np. Warszawa, Kraków, Gdańsk")]
    pub city: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CoordsQuery {
    pub name: String,
    pub latitude: f64,
    pub longitude: f64,
}

#[tool(tool_box)]
impl MeteoServer {
    pub fn new() -> Self {
        Self {
            client: Arc::new(OpenMeteoClient::new()),
        }
    }

    #[tool(description = "Pobiera pełną prognozę pogody, jakość powietrza (AQI) oraz uruchamia interaktywny panel pogodowy MCP App.")]
    async fn get_weather_dashboard(
        &self,
        #[tool(aggr)] query: WeatherQuery,
    ) -> Result<CallToolResult, McpError> {
        let report = self
            .client
            .get_full_report(&query.city)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let temp = &report.current["temperature_2m"];
        let wind = &report.current["wind_speed_10m"];
        let aqi = &report.air_quality["european_aqi"];

        let summary = format!(
            "Aktualna pogoda dla {}: {}°C, wiatr {} km/h, indeks jakości powietrza EU AQI: {}. Otwarto interaktywny wykres prognozy 7-dniowej i godzinowej.",
            report.location.name, temp, wind, aqi
        );

        let mut result = CallToolResult::success(vec![Content::text(summary)]);
        result.meta = Some(json!({
            "ui": {
                "resourceUri": UI_RESOURCE_URI
            },
            "structuredData": report
        }));

        Ok(result)
    }

    #[tool(description = "Odświeża dane pogodowe dla zadanych współrzędnych (wywoływane bezpośrednio z UI aplikacji).")]
    async fn refresh_by_coords(
        &self,
        #[tool(aggr)] coords: CoordsQuery,
    ) -> Result<CallToolResult, McpError> {
        let loc = GeoLocation {
            name: coords.name,
            country: None,
            latitude: coords.latitude,
            longitude: coords.longitude,
            timezone: None,
        };

        let report = self
            .client
            .get_report_by_coords(loc)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let mut result = CallToolResult::success(vec![Content::text("Zaktualizowano dane.")]);
        result.meta = Some(json!({
            "structuredData": report
        }));
        Ok(result)
    }
}

#[tool(tool_box)]
impl ServerHandler for MeteoServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2024_11_05,
            capabilities: ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
            server_info: Implementation {
                name: "meteo-mcp-rs".into(),
                version: env!("CARGO_PKG_VERSION").into(),
            },
            instructions: Some(
                "Serwer pogodowy Open-Meteo z interaktywnym UI (MCP App). Użyj get_weather_dashboard dla zapytań o pogodę.".into(),
            ),
        }
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        Ok(ListResourcesResult {
            resources: vec![Annotated::new(
                RawResource {
                    uri: UI_RESOURCE_URI.into(),
                    name: "Meteo Interactive Dashboard".into(),
                    description: Some("Interaktywny widżet prognozy pogody i jakości powietrza".into()),
                    mime_type: Some("text/html;profile=mcp-app".into()),
                    size: Some(DASHBOARD_HTML.len() as u32),
                },
                None,
            )],
            next_cursor: None,
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, McpError> {
        if request.uri == UI_RESOURCE_URI {
            Ok(ReadResourceResult {
                contents: vec![ResourceContents::text(
                    DASHBOARD_HTML,
                    UI_RESOURCE_URI,
                )],
            })
        } else {
            Err(McpError::resource_not_found(
                format!("Nieznany zasób: {}", request.uri),
                None,
            ))
        }
    }
}
```

---

### `src/main.rs`

```rust
mod open_meteo;
mod server;

use anyhow::Result;
use rmcp::{ServiceExt, transport::stdio};
use server::MeteoServer;
use tracing_subscriber::{EnvFilter, fmt};

#[tokio::main]
async fn main() -> Result<()> {
    // Logi wyłącznie na stderr, aby nie zakłócać komunikacji JSON-RPC na stdout
    fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let service = MeteoServer::new().serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
```

---

### `ui/dashboard.html` (Komponent MCP App)

```html
<!DOCTYPE html>
<html lang="pl">
<head>
  <meta charset="UTF-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1.0" />
  <title>Meteo MCP App</title>
  <style>
    :root {
      --bg: #0f172a;
      --card: #1e293b;
      --text: #f8fafc;
      --muted: #94a3b8;
      --accent: #38bdf8;
      --border: #334155;
    }
    * { box-sizing: border-box; margin: 0; padding: 0; font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }
    body { background: var(--bg); color: var(--text); padding: 16px; }
    .header { display: flex; justify-content: space-between; align-items: center; margin-bottom: 16px; gap: 8px; }
    .city-title { font-size: 1.4rem; font-weight: 700; }
    .subtitle { color: var(--muted); font-size: 0.85rem; }
    .search-box { display: flex; gap: 6px; }
    input { background: var(--card); border: 1px solid var(--border); color: var(--text); padding: 6px 10px; border-radius: 6px; outline: none; }
    button { background: var(--accent); color: #0f172a; border: none; padding: 6px 12px; border-radius: 6px; font-weight: 600; cursor: pointer; }
    button:hover { opacity: 0.9; }
    .metrics { display: grid; grid-template-columns: repeat(4, 1fr); gap: 10px; margin-bottom: 16px; }
    .card { background: var(--card); border: 1px solid var(--border); border-radius: 10px; padding: 12px; }
    .metric-label { font-size: 0.75rem; color: var(--muted); text-transform: uppercase; }
    .metric-val { font-size: 1.35rem; font-weight: 700; margin-top: 4px; }
    .chart-wrap { background: var(--card); border: 1px solid var(--border); border-radius: 10px; padding: 14px; margin-bottom: 16px; }
    .days { display: grid; grid-template-columns: repeat(7, 1fr); gap: 8px; }
    .day-card { background: var(--card); border: 1px solid var(--border); border-radius: 8px; padding: 8px; text-align: center; font-size: 0.8rem; }
    .day-temp { font-weight: 700; margin-top: 6px; color: var(--accent); }
    svg { width: 100%; height: 120px; overflow: visible; }
  </style>
</head>
<body>
  <div class="header">
    <div>
      <div class="city-title" id="city">Ładowanie danych...</div>
      <div class="subtitle" id="coords">Oczekiwanie na dane z serwera MCP</div>
    </div>
    <div class="search-box">
      <input type="text" id="cityInput" placeholder="Zmień miasto..." />
      <button id="searchBtn">Sprawdź</button>
    </div>
  </div>

  <div class="metrics">
    <div class="card"><div class="metric-label">Temperatura</div><div class="metric-val" id="temp">--°C</div></div>
    <div class="card"><div class="metric-label">Odczuwalna</div><div class="metric-val" id="apparent">--°C</div></div>
    <div class="card"><div class="metric-label">Wiatr</div><div class="metric-val" id="wind">-- km/h</div></div>
    <div class="card"><div class="metric-label">Jakość (EU AQI)</div><div class="metric-val" id="aqi">--</div></div>
  </div>

  <div class="chart-wrap">
    <div class="metric-label" style="margin-bottom: 8px;">Prognoza temperatury (najbliższe 24h)</div>
    <svg id="chart" viewBox="0 0 600 100"></svg>
  </div>

  <div class="days" id="days"></div>

  <script>
    let rpcId = 1;
    const pending = new Map();

    // Mostek JSON-RPC 2.0 zgodny ze specyfikacją MCP Apps
    function sendRpc(method, params = {}) {
      const id = rpcId++;
      return new Promise((resolve, reject) => {
        pending.set(id, { resolve, reject });
        window.parent.postMessage({ jsonrpc: "2.0", id, method, params }, "*");
      });
    }

    window.addEventListener("message", (event) => {
      const msg = event.data;
      if (!msg || msg.jsonrpc !== "2.0") return;

      // Odpowiedź na nasze żądanie tools/call lub ui/initialize
      if (msg.id && pending.has(msg.id)) {
        const { resolve, reject } = pending.get(msg.id);
        pending.delete(msg.id);
        if (msg.error) reject(msg.error);
        else resolve(msg.result);
        return;
      }

      // Notyfikacja z hosta z wynikiem narzędzia (ui/notifications/tool-result)
      if (msg.method === "ui/notifications/tool-result" && msg.params?._meta?.structuredData) {
        renderReport(msg.params._meta.structuredData);
      }
    });

    function renderReport(data) {
      document.getElementById("city").textContent = `${data.location.name}${data.location.country ? ', ' + data.location.country : ''}`;
      document.getElementById("coords").textContent = `${data.location.latitude.toFixed(2)}°N, ${data.location.longitude.toFixed(2)}°E`;
      document.getElementById("temp").textContent = `${data.current.temperature_2m}°C`;
      document.getElementById("apparent").textContent = `${data.current.apparent_temperature}°C`;
      document.getElementById("wind").textContent = `${data.current.wind_speed_10m} km/h`;
      document.getElementById("aqi").textContent = `${data.air_quality.european_aqi ?? 'b/d'}`;

      // Wykres SVG 24h
      const temps = data.hourly.temperature_2m.slice(0, 24);
      const min = Math.min(...temps) - 1;
      const max = Math.max(...temps) + 1;
      const points = temps.map((t, i) => {
        const x = (i / 23) * 600;
        const y = 90 - ((t - min) / (max - min || 1)) * 80;
        return `${x},${y}`;
      }).join(" ");

      document.getElementById("chart").innerHTML = `
        <polyline fill="none" stroke="#38bdf8" stroke-width="3" points="${points}" />
      `;

      // Prognoza 7-dniowa
      const daysEl = document.getElementById("days");
      daysEl.innerHTML = data.daily.time.map((date, idx) => `
        <div class="day-card">
          <div style="color: var(--muted)">${date.slice(5)}</div>
          <div class="day-temp">${Math.round(data.daily.temperature_2m_max[idx])}° / ${Math.round(data.daily.temperature_2m_min[idx])}°</div>
        </div>
      `).join("");
    }

    // Wywołanie narzędzia na serwerze MCP bezpośrednio z UI
    document.getElementById("searchBtn").addEventListener("click", async () => {
      const city = document.getElementById("cityInput").value.trim();
      if (!city) return;
      const res = await sendRpc("tools/call", {
        name: "get_weather_dashboard",
        arguments: { city }
      });
      if (res?._meta?.structuredData) {
        renderReport(res._meta.structuredData);
      }
    });

    // Inicjalizacja handshake z hostem MCP
    sendRpc("ui/initialize", {
      appInfo: { name: "meteo-dashboard", version: "0.1.0" },
      capabilities: {}
    }).catch(() => {});
  </script>
</body>
</html>
```

---

## 4. Budowanie i Konfiguracja w Kliencie MCP

1. **Kompilacja do pojedynczej binarki release:**
   ```bash
   cargo build --release
   ```
   Plik wynikowy powstanie w `./target/release/meteo-mcp-rs` (ok. 4.5 MB po `strip`).

2. **Konfiguracja w pliku JSON klienta MCP:**
   ```json
   {
     "mcpServers": {
       "meteo": {
         "command": "/ścieżka/do/meteo-mcp-rs/target/release/meteo-mcp-rs"
       }
     }
   }
   ```

3. **Testowanie lokalne bez klienta UI (MCP Inspector):**
   ```bash
   npx @modelcontextprotocol/inspector ./target/release/meteo-mcp-rs
   ```
   W inspektorze można od razu wywołać `get_weather_dashboard` dla dowolnego miasta, podejrzeć zwracane `_meta.ui.resourceUri` oraz odczytać zawartość zasobu `ui://meteo/dashboard`.