use crate::open_meteo::{GeoLocation, OpenMeteoClient, WeatherReport};
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    service::RequestContext,
    tool, tool_handler, tool_router,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

pub const DASHBOARD_HTML: &str = include_str!("../ui/dashboard.html");
pub const UI_RESOURCE_URI: &str = "ui://meteo/dashboard";
pub const UI_MIME_TYPE: &str = "text/html;profile=mcp-app";

#[derive(Clone)]
pub struct MeteoServer {
    client: Arc<OpenMeteoClient>,
    tool_router: ToolRouter<Self>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WeatherQuery {
    #[schemars(description = "City name, e.g. Warsaw, Krakow, Gdansk")]
    pub city: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CoordsQuery {
    #[schemars(description = "Location name displayed in the UI panel")]
    pub name: String,
    #[schemars(description = "Latitude")]
    pub latitude: f64,
    #[schemars(description = "Longitude")]
    pub longitude: f64,
}

fn ui_tool_meta(visibility: &[&str]) -> MetaObject {
    let mut map = serde_json::Map::new();
    map.insert(
        "ui".into(),
        json!({
            "resourceUri": UI_RESOURCE_URI,
            "visibility": visibility,
        }),
    );
    map.insert("ui/resourceUri".into(), json!(UI_RESOURCE_URI));
    MetaObject(map)
}

fn report_tool_result(
    summary: String,
    report: &WeatherReport,
    include_ui_uri: bool,
) -> Result<CallToolResult, McpError> {
    let report_json = serde_json::to_value(report).map_err(|e| {
        McpError::internal_error(format!("Failed to serialize weather report: {e}"), None)
    })?;

    let mut meta_map = serde_json::Map::new();
    if include_ui_uri {
        meta_map.insert(
            "ui".into(),
            json!({
                "resourceUri": UI_RESOURCE_URI
            }),
        );
    }
    meta_map.insert("structuredData".into(), report_json.clone());

    let mut result = CallToolResult::success(vec![ContentBlock::text(summary)]);
    result.structured_content = Some(report_json);
    result.meta = Some(MetaObject(meta_map));
    Ok(result)
}

#[tool_router]
impl MeteoServer {
    pub fn new() -> Self {
        Self::with_client(OpenMeteoClient::new())
    }

    pub fn with_client(client: OpenMeteoClient) -> Self {
        let mut tool_router = Self::tool_router();
        if let Some(route) = tool_router.map.get_mut("get_weather_dashboard") {
            route.attr.meta = Some(ui_tool_meta(&["model", "app"]));
            route.attr.annotations = Some(
                ToolAnnotations::with_title("Weather & Air Quality Forecast")
                    .read_only(true)
                    .idempotent(true)
                    .open_world(true),
            );
        }
        if let Some(route) = tool_router.map.get_mut("refresh_by_coords") {
            route.attr.meta = Some(ui_tool_meta(&["app"]));
            route.attr.annotations = Some(
                ToolAnnotations::with_title("Refresh Weather by Coordinates")
                    .read_only(true)
                    .idempotent(true)
                    .open_world(true),
            );
        }

        Self {
            client: Arc::new(client),
            tool_router,
        }
    }

    #[tool(
        name = "get_weather_dashboard",
        description = "Fetches the full weather forecast with air quality (AQI) and opens the interactive MCP App weather dashboard."
    )]
    async fn get_weather_dashboard(
        &self,
        Parameters(query): Parameters<WeatherQuery>,
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
            "Current weather for {}: {}°C, wind {} km/h, EU Air Quality Index: {}. Opened the interactive hourly and 7-day forecast dashboard.",
            report.location.name, temp, wind, aqi
        );

        report_tool_result(summary, &report, true)
    }

    #[tool(
        name = "refresh_by_coords",
        description = "Refreshes weather data for the given coordinates (invoked directly by the app UI)."
    )]
    async fn refresh_by_coords(
        &self,
        Parameters(coords): Parameters<CoordsQuery>,
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

        let summary = format!(
            "Weather data refreshed for {} ({:.2}, {:.2}).",
            report.location.name, report.location.latitude, report.location.longitude
        );

        report_tool_result(summary, &report, false)
    }
}

impl Default for MeteoServer {
    fn default() -> Self {
        Self::new()
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for MeteoServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
        .with_protocol_version(ProtocolVersion::V_2025_06_18)
        .with_server_info(Implementation::new(
            "meteo-mcp-rs",
            env!("CARGO_PKG_VERSION"),
        ))
        .with_instructions(
            "Open-Meteo weather server with an interactive UI (MCP App). Use get_weather_dashboard for weather queries.",
        )
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        let resource = Resource::new(UI_RESOURCE_URI, "Meteo Interactive Dashboard")
            .with_title("Meteo Interactive Dashboard")
            .with_description("Interactive weather forecast and air quality widget")
            .with_mime_type(UI_MIME_TYPE)
            .with_size(DASHBOARD_HTML.len() as u64);

        Ok(ListResourcesResult::with_all_items(vec![resource]))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        if request.uri == UI_RESOURCE_URI {
            let mut ui_meta = serde_json::Map::new();
            ui_meta.insert("ui".into(), json!({ "prefersBorder": true }));

            let contents = ResourceContents::text(DASHBOARD_HTML, UI_RESOURCE_URI)
                .with_mime_type(UI_MIME_TYPE)
                .with_meta(MetaObject(ui_meta));

            Ok(ReadResourceResult::new(vec![contents]).into())
        } else {
            Err(McpError::resource_not_found(
                format!("Unknown resource: {}", request.uri),
                None,
            ))
        }
    }
}
