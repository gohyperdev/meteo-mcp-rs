use meteo_mcp_rs::{
    open_meteo::OpenMeteoClient,
    server::{DASHBOARD_HTML, MeteoServer, UI_MIME_TYPE, UI_RESOURCE_URI},
};
use rmcp::{
    ClientHandler, RoleClient, RoleServer, ServerHandler, ServiceError,
    model::{CallToolRequestParams, ClientConfig, ReadResourceRequestParams, ResourceContents},
    service::serve_directly,
};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[derive(Debug, Clone, Default)]
struct DummyClient;

impl ClientHandler for DummyClient {
    fn get_info(&self) -> ClientConfig {
        ClientConfig::default()
    }
}

async fn spawn_mock_open_meteo() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                let n = socket.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let path = req
                    .lines()
                    .next()
                    .unwrap_or("")
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("/");

                // Mirror the real API: `current` echoes back ONLY the fields the
                // server actually requested in the query string. A mock that
                // unconditionally returns every field hides missing request params.
                let requested_current: Vec<String> = path
                    .split_once('?')
                    .map(|(_, q)| q)
                    .unwrap_or("")
                    .split('&')
                    .find_map(|kv| kv.strip_prefix("current="))
                    .unwrap_or("")
                    .replace("%2C", ",")
                    .split(',')
                    .map(str::to_string)
                    .collect();

                let body = if path.starts_with("/geo") {
                    json!({
                        "results": [{
                            "name": "Gdańsk",
                            "country": "Poland",
                            "latitude": 54.352,
                            "longitude": 18.6466,
                            "timezone": "Europe/Warsaw"
                        }]
                    })
                    .to_string()
                } else if path.starts_with("/forecast") {
                    let full_current = json!({
                        "temperature_2m": 16.4,
                        "relative_humidity_2m": 72,
                        "apparent_temperature": 15.8,
                        "weather_code": 2,
                        "wind_speed_10m": 18.2,
                        "surface_pressure": 1012.4,
                        "precipitation": 0.0
                    });
                    let current: serde_json::Map<String, serde_json::Value> = full_current
                        .as_object()
                        .unwrap()
                        .iter()
                        .filter(|(k, _)| requested_current.contains(k))
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    json!({
                        "current": current,
                        "hourly": {
                            "time": ["2026-08-18T12:00", "2026-08-18T13:00"],
                            "temperature_2m": [16.4, 17.1],
                            "precipitation_probability": [5, 10],
                            "weather_code": [2, 1]
                        },
                        "daily": {
                            "time": ["2026-08-18"],
                            "weather_code": [2],
                            "temperature_2m_max": [19.5],
                            "temperature_2m_min": [12.1],
                            "precipitation_sum": [0.0]
                        }
                    })
                    .to_string()
                } else {
                    json!({
                        "current": {
                            "european_aqi": 14,
                            "pm2_5": 4.2,
                            "pm10": 9.8
                        }
                    })
                    .to_string()
                };

                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
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
async fn mcp_server_exposes_tools_ui_resource_and_handles_calls() {
    let base = spawn_mock_open_meteo().await;
    let client = OpenMeteoClient::with_endpoints(
        format!("{base}/geo"),
        format!("{base}/forecast"),
        format!("{base}/aqi"),
    );
    let server_handler = MeteoServer::with_client(client);
    let server_info = server_handler.get_info();

    let (server_transport, client_transport) = tokio::io::duplex(65536);

    let server = serve_directly::<RoleServer, _, _, _, _>(
        server_handler,
        server_transport,
        Some(DummyClient.get_info()),
    );
    let server_task = tokio::spawn(async move {
        server.waiting().await.unwrap();
    });

    let mcp_client = serve_directly::<RoleClient, _, _, _, _>(
        DummyClient,
        client_transport,
        Some(server_info.into()),
    );

    // 1. Verify tools list & MCP Apps _meta.ui visibility metadata
    let tools = mcp_client.list_tools(None).await.unwrap().tools;
    assert_eq!(tools.len(), 2);

    let dashboard_tool = tools
        .iter()
        .find(|t| t.name == "get_weather_dashboard")
        .expect("get_weather_dashboard tool missing");
    let dashboard_meta = dashboard_tool.meta.as_ref().expect("tool meta missing");
    assert_eq!(
        dashboard_meta.0["ui"]["resourceUri"],
        json!(UI_RESOURCE_URI)
    );
    assert_eq!(
        dashboard_meta.0["ui"]["visibility"],
        json!(["model", "app"])
    );

    let refresh_tool = tools
        .iter()
        .find(|t| t.name == "refresh_by_coords")
        .expect("refresh_by_coords tool missing");
    let refresh_meta = refresh_tool.meta.as_ref().expect("refresh meta missing");
    assert_eq!(refresh_meta.0["ui"]["visibility"], json!(["app"]));

    // 2. Verify UI resource listing and reading
    let resources = mcp_client.list_resources(None).await.unwrap().resources;
    assert_eq!(resources.len(), 1);
    assert_eq!(resources[0].uri, UI_RESOURCE_URI);
    assert_eq!(resources[0].mime_type.as_deref(), Some(UI_MIME_TYPE));

    let read_res = mcp_client
        .read_resource(ReadResourceRequestParams::new(UI_RESOURCE_URI))
        .await
        .unwrap();
    assert_eq!(read_res.contents.len(), 1);
    match &read_res.contents[0] {
        ResourceContents::TextResourceContents {
            uri,
            mime_type,
            text,
            meta,
        } => {
            assert_eq!(uri, UI_RESOURCE_URI);
            assert_eq!(mime_type.as_deref(), Some(UI_MIME_TYPE));
            assert_eq!(text, DASHBOARD_HTML);
            assert_eq!(meta.as_ref().unwrap().0["ui"]["prefersBorder"], json!(true));
        }
        other => panic!("expected text resource contents, got: {other:?}"),
    }

    // 3. Unknown resource returns McpError
    let err = mcp_client
        .read_resource(ReadResourceRequestParams::new("ui://meteo/unknown"))
        .await
        .unwrap_err();
    assert!(matches!(err, ServiceError::McpError(_)));

    // 4. Call get_weather_dashboard
    let mut args = serde_json::Map::new();
    args.insert("city".into(), json!("Gdańsk"));
    let call_res = mcp_client
        .call_tool(CallToolRequestParams::new("get_weather_dashboard").with_arguments(args))
        .await
        .unwrap();

    assert_ne!(call_res.is_error, Some(true));
    let structured = call_res
        .structured_content
        .expect("structured_content should be set");
    assert_eq!(structured["location"]["name"], json!("Gdańsk"));
    assert_eq!(structured["current"]["temperature_2m"], json!(16.4));
    // The dashboard UI renders `current.precipitation` unconditionally
    // (ui/dashboard.html), so the server MUST request it from Open-Meteo.
    assert_eq!(structured["current"]["precipitation"], json!(0.0));
    assert_eq!(structured["air_quality"]["european_aqi"], json!(14));
    assert_eq!(
        call_res.meta.as_ref().unwrap().0["ui"]["resourceUri"],
        json!(UI_RESOURCE_URI)
    );

    // 5. Call app-only refresh_by_coords
    let mut coords_args = serde_json::Map::new();
    coords_args.insert("name".into(), json!("Berlin"));
    coords_args.insert("latitude".into(), json!(52.52));
    coords_args.insert("longitude".into(), json!(13.405));
    let refresh_res = mcp_client
        .call_tool(CallToolRequestParams::new("refresh_by_coords").with_arguments(coords_args))
        .await
        .unwrap();

    let refresh_structured = refresh_res.structured_content.unwrap();
    assert_eq!(refresh_structured["location"]["name"], json!("Berlin"));
    assert_eq!(refresh_structured["location"]["latitude"], json!(52.52));

    drop(mcp_client);
    let _ = server_task.await;
}
