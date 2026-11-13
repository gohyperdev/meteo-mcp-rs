# meteo-mcp-rs

[![CI](https://github.com/gohyperdev/meteo-mcp-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/gohyperdev/meteo-mcp-rs/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Built with Rust](https://img.shields.io/badge/Built%20with-Rust-orange.svg)](https://www.rust-lang.org/)

A fast, zero-API-key **Model Context Protocol (MCP)** weather server written in **Rust**, featuring an interactive **MCP App** (`ui://meteo/dashboard`) powered by the [Open-Meteo](https://open-meteo.com/) API.

Ask Claude (or any MCP Apps–capable host) for the weather and get an interactive dashboard with current telemetry, European Air Quality Index (PM2.5/PM10), a 24-hour temperature & precipitation chart, and a 7-day forecast — rendered directly inside the conversation with zero API keys or external Node/Python runtimes.

## Features

- **Single self-contained binary**: `ui/dashboard.html` is embedded at compile time (`include_str!`), zero runtime asset dependencies.
- **No API keys required**: Uses Open-Meteo's free Geocoding, Forecast, and Air Quality APIs.
- **MCP Apps (`2026-01-26`) compliant**:
  - Serves UI resource `ui://meteo/dashboard` with MIME type `text/html;profile=mcp-app`.
  - Attaches `_meta.ui.resourceUri` and `_meta.ui.visibility` metadata to tools.
  - Supports host theme variables (`light` / `dark`), `ResizeObserver` auto-sizing, and inline/fullscreen display modes.
- **Interactive UI-to-Server refreshes**:
  - `get_weather_dashboard` (`visibility: ["model", "app"]`): Geocodes a city name and returns full weather + air quality telemetry.
  - `refresh_by_coords` (`visibility: ["app"]`): App-only hidden tool for instant coordinate-based refreshes and city preset pills without cluttering the LLM's tool list.
- **Graceful degradation**: If air quality telemetry is temporarily unavailable, the weather forecast still succeeds.

## Architecture

```text
┌──────────────────────────────┐        stdio (JSON-RPC)        ┌──────────────────────────────┐
│   MCP Host (Claude / IDE)    │ ◄────────────────────────────► │       meteo-mcp-rs           │
│                              │                                │                              │
│  ┌────────────────────────┐  │  tools/call + structuredData   │  • OpenMeteoClient (reqwest) │
│  │  ui://meteo/dashboard  │  │ ◄────────────────────────────► │  • MeteoServer (rmcp 3.10)   │
│  │  (Sandboxed Iframe)    │  │  resources/read (MCP App HTML) │  • Embedded ui/dashboard.html│
│  └────────────────────────┘  │                                └──────────────┬───────────────┘
└──────────────────────────────┘                                               │ HTTPS (rustls)
                                                                               ▼
                                                                    Open-Meteo Public APIs
                                                             (Geocoding / Forecast / Air Quality)
```

## Quick Start

### Download prebuilt binaries (recommended)

Grab the latest archive for your platform from [GitHub Releases](https://github.com/gohyperdev/meteo-mcp-rs/releases/latest) (each release also includes `SHA256SUMS.txt`):

| Platform | Architecture | Archive |
| --- | --- | --- |
| macOS | Apple Silicon (`arm64`) | `meteo-mcp-rs-<version>-aarch64-apple-darwin.tar.gz` |
| macOS | Intel (`x86_64`) | `meteo-mcp-rs-<version>-x86_64-apple-darwin.tar.gz` |
| Linux | `x86_64` | `meteo-mcp-rs-<version>-x86_64-unknown-linux-gnu.tar.gz` |
| Linux | `arm64` (`aarch64`) | `meteo-mcp-rs-<version>-aarch64-unknown-linux-gnu.tar.gz` |
| Windows | `x86_64` | `meteo-mcp-rs-<version>-x86_64-pc-windows-msvc.zip` |

Example install on macOS / Linux (replace `<version>` & `<target>`):

```bash
curl -fsSL "https://github.com/gohyperdev/meteo-mcp-rs/releases/latest/download/meteo-mcp-rs-v0.1.0-aarch64-apple-darwin.tar.gz" | tar -xz
sudo mv meteo-mcp-rs-v0.1.0-aarch64-apple-darwin/meteo-mcp-rs /usr/local/bin/
```

On macOS, if Gatekeeper blocks an unsigned downloaded binary:

```bash
xattr -d com.apple.quarantine /usr/local/bin/meteo-mcp-rs
```

### Build from source

Requires Rust toolchain (`rustc` 1.88+ / edition 2024).

```bash
git clone https://github.com/gohyperdev/meteo-mcp-rs.git
cd meteo-mcp-rs
cargo build --release
```

The binary is compiled to `target/release/meteo-mcp-rs`.

### Install via Cargo

```bash
cargo install --git https://github.com/gohyperdev/meteo-mcp-rs
```

### Configure in Claude Desktop

Add to your `claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "meteo": {
      "command": "/absolute/path/to/meteo-mcp-rs"
    }
  }
}
```

### Configure in Claude Code

```bash
claude mcp add meteo /absolute/path/to/meteo-mcp-rs
```

### Test with MCP Inspector

```bash
npx @modelcontextprotocol/inspector ./target/release/meteo-mcp-rs
```

## Tools & Resources

### Tools

| Tool | Visibility | Arguments | Description |
| --- | --- | --- | --- |
| `get_weather_dashboard` | `["model", "app"]` | `city: string` | Geocodes a city and returns current weather, 24h hourly, 7-day forecast, and EU AQI telemetry. |
| `refresh_by_coords` | `["app"]` | `name: string`, `latitude: number`, `longitude: number` | App-only tool used by the interactive UI to refresh or switch preset cities without geocoding. |

### Resources

| URI | MIME Type | Description |
| --- | --- | --- |
| `ui://meteo/dashboard` | `text/html;profile=mcp-app` | Self-contained HTML/CSS/SVG/JS interactive weather dashboard (`include_str!`). |

## Development

```bash
# Check formatting
cargo fmt --all -- --check

# Lint
cargo clippy --all-targets -- -D warnings

# Run unit & mock-HTTP integration tests
cargo test

# Run with debug logs (written to stderr; stdout stays reserved for MCP JSON-RPC)
RUST_LOG=meteo_mcp_rs=debug cargo run
```

## License

MIT — see [LICENSE](LICENSE).
