#[path = "../client_accounts.rs"]
mod client_accounts;
#[path = "../client_adapters/mod.rs"]
mod client_adapters;
#[path = "../client_codex_catalog.rs"]
mod client_codex_catalog;
#[path = "../client_common.rs"]
mod client_common;
#[path = "../client_config_validation.rs"]
mod client_config_validation;
#[path = "../client_connect.rs"]
mod client_connect;
#[path = "../client_handler.rs"]
mod client_handler;
#[path = "../client_install.rs"]
mod client_install;
#[path = "../client_model_capabilities.rs"]
mod client_model_capabilities;
#[path = "../client_model_fetch.rs"]
mod client_model_fetch;
#[path = "../client_proxy.rs"]
mod client_proxy;
#[path = "../client_usage.rs"]
mod client_usage;

use std::{collections::BTreeMap, env, fs, io::Read, path::Path, path::PathBuf};

use anyhow::{bail, Context, Result};
use axum::{
    extract::{Query, State},
    http::{HeaderValue, Method},
    routing::{get, post},
    Json, Router,
};
use client_connect::{Client, ConnectRequest};
use serde::{Deserialize, Serialize};
use tower_http::cors::{AllowOrigin, CorsLayer};
use url::Url;

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("错误：{error:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let mut args = env::args().skip(1);
    let command = args.next().unwrap_or_else(|| "help".into());
    let rest = args.collect::<Vec<_>>();
    if command.starts_with("junohub://") {
        return handle_deep_link(&command).await;
    }
    match command.as_str() {
        "apply" => apply(&rest).await,
        "setup" => setup(&rest).await,
        "inspect" => inspect(&rest).await,
        "serve" => serve(&rest).await,
        "install" => tool_action(&rest, "install").await,
        "update" => tool_action(&rest, "update").await,
        "install-handler" => install_handler(&rest),
        "restore" => restore(&rest),
        "help" | "--help" | "-h" => {
            print_help();
            Ok(())
        }
        _ => bail!("未知命令：{command}。运行 junohub-connect --help 查看用法"),
    }
}

#[derive(Clone)]
struct HelperState {
    home: PathBuf,
    proxy: client_proxy::LocalProxy,
}

#[derive(Default, Deserialize)]
struct InspectQuery {
    client: Option<String>,
}

#[derive(Serialize)]
struct HealthResult {
    ok: bool,
}

async fn serve(args: &[String]) -> Result<()> {
    let port = option(args, "--port")
        .map(|value| value.parse::<u16>().context("--port 必须是有效端口"))
        .transpose()?
        .unwrap_or(client_handler::LOCAL_HELPER_PORT);
    let home = option(args, "--home")
        .map(PathBuf::from)
        .map(Ok)
        .unwrap_or_else(client_connect::default_home)?;
    let allowed_origins = AllowOrigin::predicate(|origin: &HeaderValue, _| {
        let value = origin.to_str().unwrap_or_default();
        value
            .strip_prefix("http://localhost:")
            .is_some_and(valid_origin_port)
            || value
                .strip_prefix("http://127.0.0.1:")
                .is_some_and(valid_origin_port)
            || value
                .strip_prefix("https://localhost:")
                .is_some_and(valid_origin_port)
            || value
                .strip_prefix("https://127.0.0.1:")
                .is_some_and(valid_origin_port)
    });
    let proxy = client_proxy::LocalProxy::new(home.clone(), port)?;
    let app = Router::new()
        .route(
            "/accounts",
            get(client_accounts::status).post(client_accounts::action),
        )
        .route("/proxy/status", get(client_proxy::status))
        .route("/usage", get(client_usage::history))
        .route("/proxy/probe", post(client_proxy::probe))
        .route("/proxy/configure", post(client_proxy::configure))
        .route("/proxy/settings", post(client_proxy::settings))
        .route("/proxy/queue", post(client_proxy::queue))
        .route("/proxy/disable", post(client_proxy::disable))
        .route(
            "/local-proxy/{client}/{*path}",
            get(client_proxy::forward).post(client_proxy::forward),
        )
        .route("/health", get(|| async { Json(HealthResult { ok: true }) }))
        .route("/inspect", get(inspect_handler))
        .route("/providers", get(providers_handler))
        .route("/configuration-status", get(configuration_status_handler))
        .route("/models", post(models_handler))
        .route("/tool", post(tool_handler))
        .route("/connect", post(connect_handler))
        .route("/adopt", post(adopt_handler))
        .route("/remove-provider", post(remove_provider_handler))
        .layer(
            CorsLayer::new()
                .allow_origin(allowed_origins)
                .allow_headers([axum::http::header::CONTENT_TYPE])
                .allow_methods([Method::GET, Method::POST]),
        )
        .with_state(HelperState { home, proxy });
    let address = format!("127.0.0.1:{port}");
    let listener = tokio::net::TcpListener::bind(&address)
        .await
        .with_context(|| format!("无法启动本机检测接口 {address}"))?;
    println!("JunoHub 本机检测接口已启动：http://{address}");
    axum::serve(listener, app)
        .await
        .context("本机检测接口异常退出")
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelsRequest {
    #[serde(default)]
    auth_binding: Option<client_accounts::Binding>,
    base_url: String,
    #[serde(default)]
    api_key: String,
    #[serde(default)]
    api_format: Option<String>,
    #[serde(default)]
    request_headers: BTreeMap<String, String>,
    #[serde(default)]
    models_url: Option<String>,
}

async fn models_handler(
    State(state): State<HelperState>,
    Json(request): Json<ModelsRequest>,
) -> Json<serde_json::Value> {
    if let Some(binding) = &request.auth_binding {
        return Json(match state.proxy.accounts.models(binding).await {
            Ok(models) => serde_json::json!({"success":true,"models":models}),
            Err(e) => serde_json::json!({"success":false,"error":e.to_string()}),
        });
    }
    let valid_url = |raw: &str| {
        Url::parse(raw).is_ok_and(|url| {
            matches!(url.scheme(), "http" | "https")
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
        })
    };
    if !valid_url(&request.base_url)
        || request
            .models_url
            .as_deref()
            .is_some_and(|url| !url.is_empty() && !valid_url(url))
    {
        return Json(
            serde_json::json!({"success":false,"error":"请输入有效的 HTTP 或 HTTPS 地址"}),
        );
    }
    match client_model_fetch::fetch_models(
        &request.base_url,
        &request.api_key,
        false,
        request.models_url.as_deref(),
        None,
        request.api_format.as_deref(),
        Some(&request.request_headers),
    )
    .await
    {
        Ok(models) => Json(serde_json::json!({"success":true,"models":models})),
        Err(error) => Json(serde_json::json!({"success":false,"error":error})),
    }
}

#[derive(Deserialize)]
struct ToolRequest {
    client: String,
    action: String,
}

async fn tool_handler(
    State(state): State<HelperState>,
    Json(request): Json<ToolRequest>,
) -> Json<NativeResult> {
    let clients = match Client::parse(&request.client) {
        Ok(clients) => clients,
        Err(error) => {
            return Json(NativeResult {
                action: request.action,
                success: false,
                tools: Vec::new(),
                error: Some(error.to_string()),
                backup_id: None,
                active_providers: verified_active_providers(&state.home),
            });
        }
    };
    if !matches!(request.action.as_str(), "install" | "update") {
        return Json(NativeResult {
            action: request.action,
            success: false,
            tools: Vec::new(),
            error: Some("不支持的客户端操作".into()),
            backup_id: None,
            active_providers: verified_active_providers(&state.home),
        });
    }
    let mut tools = Vec::new();
    let mut errors = Vec::new();
    for client in clients {
        let operation = if request.action == "install" {
            client_install::install_client(client, &state.home).await
        } else {
            client_install::update_client(client, &state.home).await
        };
        match operation {
            Ok(status) => tools.push(status),
            Err(error) => {
                errors.push(format!("{}：{error:#}", client.label()));
                tools.extend(client_install::inspect_clients(&[client], &state.home).await);
            }
        }
    }
    Json(NativeResult {
        action: request.action,
        success: errors.is_empty(),
        tools,
        error: (!errors.is_empty()).then(|| errors.join("；")),
        backup_id: None,
        active_providers: verified_active_providers(&state.home),
    })
}

#[derive(Deserialize)]
struct ConnectPayload {
    pair_url: String,
}

#[derive(Deserialize)]
struct AdoptPayload {
    client: String,
    id: String,
    name: String,
}

#[derive(Deserialize)]
struct RemoveProviderPayload {
    client: String,
    id: String,
    provider_key: String,
}

async fn remove_provider_handler(
    State(state): State<HelperState>,
    Json(input): Json<RemoveProviderPayload>,
) -> Json<NativeResult> {
    let result = (|| -> Result<client_connect::ConnectResult> {
        let clients = Client::parse(&input.client)?;
        if clients.len() != 1 {
            bail!("请选择一个客户端");
        }
        if client_proxy::is_taken_over(&state.home, clients[0].key())? {
            bail!("当前客户端由本地代理接管，请先停用代理再移除供应商");
        }
        let result = client_connect::native::remove_native_provider(
            &state.home,
            clients[0],
            &input.provider_key,
        )?;
        let mut management = read_management_state(&state.home);
        management
            .active_providers
            .remove(&format!("{}:{}", clients[0].key(), input.id));
        fs::write(
            management_state_path(&state.home),
            serde_json::to_vec_pretty(&management)?,
        )?;
        Ok(result)
    })();
    Json(NativeResult {
        action: "connect".into(),
        success: result.is_ok(),
        tools: vec![],
        backup_id: result.as_ref().ok().map(|r| r.backup_id.clone()),
        error: result.err().map(|e| e.to_string()),
        active_providers: verified_active_providers(&state.home),
    })
}

async fn adopt_handler(
    State(state): State<HelperState>,
    Json(input): Json<AdoptPayload>,
) -> Json<NativeResult> {
    let result = Client::parse(&input.client).and_then(|clients| {
        if clients.len() != 1 {
            bail!("一次只能导入一个客户端");
        }
        record_active_provider(&state.home, &input.id, &input.name, "", &clients)
    });
    match result {
        Ok(active_providers) => Json(NativeResult {
            action: "connect".into(),
            success: true,
            tools: vec![],
            error: None,
            backup_id: None,
            active_providers,
        }),
        Err(error) => Json(NativeResult {
            action: "connect".into(),
            success: false,
            tools: vec![],
            error: Some(error.to_string()),
            backup_id: None,
            active_providers: verified_active_providers(&state.home),
        }),
    }
}

async fn connect_handler(
    State(state): State<HelperState>,
    Json(request): Json<ConnectPayload>,
) -> Json<NativeResult> {
    let result =
        match connect_pairing(&request.pair_url, state.home.clone(), Some(&state.proxy)).await {
            Ok((result, provider_id, provider_name, clients)) => {
                match record_active_provider(
                    &state.home,
                    &provider_id,
                    &provider_name,
                    &result.backup_id,
                    &clients,
                ) {
                    Ok(active_providers) => NativeResult {
                        action: "connect".into(),
                        success: true,
                        tools: Vec::new(),
                        error: None,
                        backup_id: Some(result.backup_id),
                        active_providers,
                    },
                    Err(error) => NativeResult {
                        action: "connect".into(),
                        success: false,
                        tools: Vec::new(),
                        error: Some(format!("{error:#}")),
                        backup_id: None,
                        active_providers: verified_active_providers(&state.home),
                    },
                }
            }
            Err(error) => NativeResult {
                action: "connect".into(),
                success: false,
                tools: Vec::new(),
                error: Some(format!("{error:#}")),
                backup_id: None,
                active_providers: verified_active_providers(&state.home),
            },
        };
    Json(result)
}

#[derive(Serialize)]
struct ImportedProvidersResult {
    success: bool,
    providers: Vec<client_connect::native::NativeImport>,
    error: Option<String>,
}

async fn providers_handler(
    State(state): State<HelperState>,
    Query(query): Query<InspectQuery>,
) -> Json<ImportedProvidersResult> {
    let client = match Client::parse(query.client.as_deref().unwrap_or("")) {
        Ok(clients) if clients.len() == 1 => clients[0],
        _ => {
            return Json(ImportedProvidersResult {
                success: false,
                providers: Vec::new(),
                error: Some("请选择一个客户端导入配置".into()),
            });
        }
    };
    if client_proxy::is_taken_over(&state.home, client.key()).unwrap_or(true) {
        return Json(ImportedProvidersResult {
            success: false,
            providers: vec![],
            error: Some(
                "当前客户端已由本地路由接管，请停用并恢复后导入，避免把代理地址当作供应商。".into(),
            ),
        });
    }
    match client_connect::read_native_providers(&state.home, client) {
        Ok(providers) => Json(ImportedProvidersResult {
            success: true,
            providers,
            error: None,
        }),
        Err(error) => Json(ImportedProvidersResult {
            success: false,
            providers: Vec::new(),
            error: Some(format!("{error:#}")),
        }),
    }
}

fn valid_origin_port(value: &str) -> bool {
    !value.is_empty() && value.chars().all(|character| character.is_ascii_digit())
}

// Reading configured providers never runs a CLI or looks up available versions.
async fn configuration_status_handler(State(state): State<HelperState>) -> Json<NativeResult> {
    Json(NativeResult {
        action: "configuration-status".into(),
        success: true,
        tools: Vec::new(),
        error: None,
        backup_id: None,
        active_providers: verified_active_providers(&state.home),
    })
}

async fn inspect_handler(
    State(state): State<HelperState>,
    Query(query): Query<InspectQuery>,
) -> Json<NativeResult> {
    let clients = match query.client.as_deref().unwrap_or("all") {
        "all" => all_clients(),
        value => match Client::parse(value) {
            Ok(clients) => clients,
            Err(error) => {
                return Json(NativeResult {
                    action: "inspect".into(),
                    success: false,
                    tools: Vec::new(),
                    error: Some(error.to_string()),
                    backup_id: None,
                    active_providers: verified_active_providers(&state.home),
                });
            }
        },
    };
    Json(NativeResult {
        action: "inspect".into(),
        success: true,
        tools: client_install::inspect_clients(&clients, &state.home).await,
        error: None,
        backup_id: None,
        active_providers: verified_active_providers(&state.home),
    })
}

fn all_clients() -> Vec<Client> {
    vec![
        Client::Claude,
        Client::Codex,
        Client::Gemini,
        Client::Grok,
        Client::OpenCode,
        Client::OpenClaw,
        Client::Hermes,
        Client::Pi,
    ]
}

async fn inspect(args: &[String]) -> Result<()> {
    let clients = Client::parse(&option(args, "--client").unwrap_or_else(|| "all".into()))?;
    let home = option(args, "--home")
        .map(PathBuf::from)
        .map(Ok)
        .unwrap_or_else(client_connect::default_home)?;
    let statuses = client_install::inspect_clients(&clients, &home).await;
    println!("{}", serde_json::to_string_pretty(&statuses)?);
    Ok(())
}

async fn tool_action(args: &[String], action: &str) -> Result<()> {
    let clients = Client::parse(&required(args, "--client")?)?;
    let home = option(args, "--home")
        .map(PathBuf::from)
        .map(Ok)
        .unwrap_or_else(client_connect::default_home)?;
    for client in clients {
        let status = if action == "install" {
            client_install::install_client(client, &home).await?
        } else {
            client_install::update_client(client, &home).await?
        };
        println!(
            "{}：{}",
            status.label,
            status.current_version.as_deref().unwrap_or("无法运行")
        );
    }
    Ok(())
}

#[derive(Serialize)]
struct NativeResult {
    action: String,
    success: bool,
    tools: Vec<client_install::ToolStatus>,
    error: Option<String>,
    backup_id: Option<String>,
    active_providers: BTreeMap<String, ActiveProvider>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
struct ActiveProvider {
    id: String,
    name: String,
    backup_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    config_fingerprint: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct LocalManagementState {
    active_providers: BTreeMap<String, ActiveProvider>,
}

async fn handle_deep_link(value: &str) -> Result<()> {
    let host = client_handler::deep_link_host(value)?;
    let return_url = client_handler::deep_link_param(value, "return_url").ok();
    client_handler::notify("正在处理客户端集成");
    let home = client_connect::default_home()?;
    let result = match host.as_str() {
        "inspect" => NativeResult {
            action: "inspect".into(),
            success: true,
            tools: client_install::inspect_clients(&all_clients(), &home).await,
            error: None,
            backup_id: None,
            active_providers: verified_active_providers(&home),
        },
        "tool" => {
            let client_value = client_handler::deep_link_param(value, "client")?;
            let clients = client_value
                .split(',')
                .map(Client::parse)
                .collect::<Result<Vec<_>>>()?
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
            let action = client_handler::deep_link_param(value, "action")?;
            if !matches!(action.as_str(), "install" | "update") {
                bail!("不支持的客户端操作：{action}");
            }
            let mut tools = Vec::new();
            let mut errors = Vec::new();
            for client in &clients {
                let operation = if action == "install" {
                    client_install::install_client(*client, &home).await
                } else {
                    client_install::update_client(*client, &home).await
                };
                match operation {
                    Ok(status) => tools.push(status),
                    Err(error) => {
                        errors.push(format!("{}：{error:#}", client.label()));
                        tools.extend(client_install::inspect_clients(&[*client], &home).await);
                    }
                }
            }
            NativeResult {
                action,
                success: errors.is_empty(),
                tools,
                error: (!errors.is_empty()).then(|| errors.join("；")),
                backup_id: None,
                active_providers: verified_active_providers(&home),
            }
        }
        "connect" => {
            let pair_url = client_handler::pairing_url_from_deep_link(value)?;
            match connect_pairing(&pair_url, home.clone(), None).await {
                Ok((result, provider_id, provider_name, clients)) => {
                    let active_providers = record_active_provider(
                        &home,
                        &provider_id,
                        &provider_name,
                        &result.backup_id,
                        &clients,
                    )?;
                    NativeResult {
                        action: "connect".into(),
                        success: true,
                        tools: Vec::new(),
                        error: None,
                        backup_id: Some(result.backup_id),
                        active_providers,
                    }
                }
                Err(error) => NativeResult {
                    action: "connect".into(),
                    success: false,
                    tools: Vec::new(),
                    error: Some(format!("{error:#}")),
                    backup_id: None,
                    active_providers: verified_active_providers(&home),
                },
            }
        }
        _ => bail!("不支持的 JunoHub 集成操作：{host}"),
    };
    if let Some(return_url) = return_url {
        client_handler::open_result(&return_url, &result)?;
    }
    if result.success {
        client_handler::notify("客户端集成操作已完成");
        Ok(())
    } else {
        let error = result.error.unwrap_or_else(|| "未知错误".into());
        client_handler::notify(&format!("客户端集成失败：{error}"));
        bail!("{error}")
    }
}

async fn connect_pairing(
    pair_url: &str,
    home: PathBuf,
    proxy: Option<&client_proxy::LocalProxy>,
) -> Result<(client_connect::ConnectResult, String, String, Vec<Client>)> {
    let mut pairing = redeem_pairing(pair_url).await?;
    client_common::apply_pair(&home, &mut pairing)?;
    let clients = Client::parse(&pairing.client)?;
    let provider_id = pairing.provider_id.clone();
    let provider_name = pairing.name.clone();
    let needs_conversion = pairing
        .models
        .as_ref()
        .and_then(|m| m.native.as_ref())
        .is_some_and(|n| {
            n.auth_binding.is_some()
                || match n.client.as_str() {
                    "claude" => n
                        .api_format
                        .as_deref()
                        .is_some_and(|format| format != "anthropic"),
                    "codex" => n
                        .api_format
                        .as_deref()
                        .is_some_and(|format| !matches!(format, "openai_responses" | "responses")),
                    _ => false,
                }
        });
    let taken = clients.len() == 1 && client_proxy::is_taken_over(&home, clients[0].key())?;
    let result = if taken {
        proxy
            .context("请通过网页切换已接管的供应商")?
            .switch_pair(pairing)
            .await?
    } else if needs_conversion {
        proxy
            .context("此供应商需要协议转换，请通过网页启用供应商并保持本地组件运行")?
            .configure_pairs(
                client_proxy::Configure {
                    client: clients[0].key().into(),
                    pair_urls: vec![],
                    headers: None,
                    body: None,
                    timeout_seconds: 30,
                    failure_threshold: 3,
                    cooldown_seconds: 30,
                    auto_failover_enabled: Some(false),
                },
                vec![pairing],
            )
            .await?
    } else {
        apply_pairing(pairing, clients.clone(), home)?
    };
    print_connect_result(&result);
    Ok((result, provider_id, provider_name, clients))
}

async fn setup(args: &[String]) -> Result<()> {
    let pair_url = required(args, "--pair-url")?;
    let home = option(args, "--home").map(PathBuf::from);
    setup_pairing(&pair_url, home).await
}

async fn setup_pairing(pair_url: &str, home: Option<PathBuf>) -> Result<()> {
    let pairing = redeem_pairing(pair_url).await?;
    let clients = Client::parse(&pairing.client)?;
    let home = home.map(Ok).unwrap_or_else(client_connect::default_home)?;
    println!("正在检测客户端，未安装的客户端将自动安装……");
    let outcomes = client_install::ensure_clients(&clients, &home).await;
    let mut failures = Vec::new();
    for (client, outcome) in clients.iter().zip(outcomes) {
        match outcome {
            Ok(outcome) => println!(
                "{}：{}（{}）",
                outcome.client.label(),
                if outcome.installed_now {
                    "已安装"
                } else {
                    "已存在"
                },
                outcome.version
            ),
            Err(error) => {
                eprintln!("{}：安装失败：{error:#}", client.label());
                failures.push(format!("{}：{error:#}", client.label()));
            }
        }
    }
    let result = apply_pairing(pairing, clients, home)?;
    print_connect_result(&result);
    if failures.is_empty() {
        Ok(())
    } else {
        bail!(
            "配置已经写入，但以下客户端未能完成安装：{}",
            failures.join("；")
        )
    }
}

async fn apply(args: &[String]) -> Result<()> {
    let pairing = if let Some(pair_url) = option(args, "--pair-url") {
        Some(redeem_pairing(&pair_url).await?)
    } else {
        None
    };
    let client = pairing
        .as_ref()
        .map(|pairing| pairing.client.clone())
        .or_else(|| option(args, "--client"))
        .unwrap_or_else(|| "all".into());
    let base_url = pairing
        .as_ref()
        .map(|pairing| pairing.base_url.clone())
        .map(Ok)
        .unwrap_or_else(|| required(args, "--base-url"))?;
    let model = pairing
        .as_ref()
        .map(|pairing| pairing.model.clone())
        .map(Ok)
        .unwrap_or_else(|| required(args, "--model"))?;
    let home = option(args, "--home")
        .map(PathBuf::from)
        .map(Ok)
        .unwrap_or_else(client_connect::default_home)?;
    let api_key = if let Some(pairing) = pairing {
        pairing.api_key
    } else if has_flag(args, "--api-key-stdin") {
        let mut value = String::new();
        std::io::stdin()
            .read_to_string(&mut value)
            .context("无法从标准输入读取访问密钥")?;
        value.trim_end_matches(['\r', '\n']).to_string()
    } else {
        env::var("JUNOHUB_API_KEY")
            .context("请通过 JUNOHUB_API_KEY 或 --api-key-stdin 提供访问密钥")?
    };
    let result = client_connect::apply(ConnectRequest {
        clients: Client::parse(&client)?,
        provider_id: "managed".into(),
        provider_name: "JunoHub".into(),
        base_url,
        api_key,
        model,
        models: None,
        home,
    })?;
    print_connect_result(&result);
    Ok(())
}

fn apply_pairing(
    pairing: PairData,
    clients: Vec<Client>,
    home: PathBuf,
) -> Result<client_connect::ConnectResult> {
    for client in &clients {
        if client_proxy::is_taken_over(&home, client.key())? {
            bail!("当前客户端已由本地代理接管，请在本地路由中更新供应商或先停用代理");
        }
    }
    client_connect::apply(ConnectRequest {
        clients,
        provider_id: pairing.provider_id,
        provider_name: pairing.name,
        base_url: pairing.base_url,
        api_key: pairing.api_key,
        model: pairing.model,
        models: pairing.models,
        home,
    })
}

fn print_connect_result(result: &client_connect::ConnectResult) {
    println!("已接入：{}", result.clients.join("、"));
    for file in &result.files {
        println!("  {file}");
    }
    println!("备份编号：{}", result.backup_id);
    println!(
        "恢复命令：junohub-connect restore --backup {}",
        result.backup_id
    );
}

fn install_handler(args: &[String]) -> Result<()> {
    let home = option(args, "--home")
        .map(PathBuf::from)
        .map(Ok)
        .unwrap_or_else(client_connect::default_home)?;
    let path = client_handler::install(&home)?;
    println!("JunoHub 一键接入组件已启用：{path}");
    Ok(())
}

#[derive(Deserialize)]
struct PairEnvelope {
    code: u16,
    message: Option<String>,
    data: Option<PairData>,
}

#[derive(Deserialize)]
struct PairData {
    client: String,
    model: String,
    #[serde(default = "default_provider_name")]
    name: String,
    #[serde(default)]
    provider_id: String,
    #[serde(default)]
    models: Option<client_connect::UniversalModels>,
    api_key: String,
    #[serde(default)]
    base_url: String,
}

fn management_state_path(home: &Path) -> PathBuf {
    home.join(".junohub/management.json")
}

fn read_management_state(home: &Path) -> LocalManagementState {
    fs::read(management_state_path(home))
        .ok()
        .and_then(|value| serde_json::from_slice(&value).ok())
        .unwrap_or_default()
}

fn native_config_fingerprint(home: &Path, client: &str) -> Option<String> {
    use sha2::{Digest, Sha256};
    let paths: &[&str] = match client {
        "claude" => &[".claude/settings.json"],
        "codex" => &[".codex/config.toml"],
        "gemini" => &[".gemini/.env", ".gemini/settings.json"],
        "grok" => &[".grok/config.toml"],
        _ => return None,
    };
    let mut hash = Sha256::new();
    let mut found = false;
    for path in paths {
        hash.update(path.as_bytes());
        match fs::read(home.join(path)) {
            Ok(bytes) => {
                found = true;
                hash.update([1]);
                hash.update(bytes)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => hash.update([0]),
            Err(_) => return None,
        }
    }
    found.then(|| format!("{:x}", hash.finalize()))
}
fn verified_active_providers(home: &Path) -> BTreeMap<String, ActiveProvider> {
    read_management_state(home)
        .active_providers
        .into_iter()
        .filter(|(key, provider)| {
            if key.contains(':') {
                return true;
            } // Additive clients use “added”, not an exclusive active provider.
            provider
                .config_fingerprint
                .as_ref()
                .is_some_and(|expected| {
                    native_config_fingerprint(home, key).as_ref() == Some(expected)
                })
        })
        .collect()
}
fn record_active_provider(
    home: &Path,
    provider_id: &str,
    provider_name: &str,
    backup_id: &str,
    clients: &[Client],
) -> Result<BTreeMap<String, ActiveProvider>> {
    let mut state = read_management_state(home);
    for client in clients {
        let key = if matches!(
            client,
            Client::OpenCode | Client::OpenClaw | Client::Hermes | Client::Pi
        ) {
            format!("{}:{provider_id}", client.key())
        } else {
            client.key().into()
        };
        state.active_providers.insert(
            key,
            ActiveProvider {
                id: provider_id.into(),
                name: provider_name.into(),
                backup_id: backup_id.into(),
                config_fingerprint: native_config_fingerprint(home, client.key()),
            },
        );
    }
    let path = management_state_path(home);
    fs::create_dir_all(path.parent().context("本机管理目录无效")?)?;
    client_connect::atomic_write(&path, &serde_json::to_vec_pretty(&state)?)?;
    Ok(verified_active_providers(home))
}

fn default_provider_name() -> String {
    "JunoHub".into()
}

async fn redeem_pairing(pair_url: &str) -> Result<PairData> {
    let mut url = Url::parse(pair_url).context("接入链接无效")?;
    if !matches!(url.scheme(), "http" | "https") {
        bail!("接入链接必须使用 HTTP 或 HTTPS");
    }
    let token = url
        .query_pairs()
        .find(|(key, _)| key == "token")
        .map(|(_, value)| value.into_owned())
        .filter(|value| !value.is_empty())
        .context("接入链接缺少一次性凭证")?;
    url.set_query(None);
    let origin = url.origin().ascii_serialization();
    let response = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(10))
        .build()?
        .post(url)
        .json(&serde_json::json!({"token": token}))
        .send()
        .await
        .context("无法连接 JunoHub")?;
    let status = response.status();
    let envelope: PairEnvelope = response.json().await.context("JunoHub 返回了无效数据")?;
    if !status.is_success() || envelope.code != 200 {
        bail!(
            "{}",
            envelope
                .message
                .unwrap_or_else(|| format!("JunoHub 返回 HTTP {status}"))
        );
    }
    let mut data = envelope.data.context("JunoHub 未返回接入配置")?;
    if data.base_url.trim().is_empty() {
        data.base_url = origin;
    }
    Ok(data)
}

fn restore(args: &[String]) -> Result<()> {
    let backup = required(args, "--backup")?;
    let home = option(args, "--home")
        .map(PathBuf::from)
        .map(Ok)
        .unwrap_or_else(client_connect::default_home)?;
    client_proxy::check_restore(&home, &backup)?;
    let files = client_connect::restore(&home, &backup)?;
    println!("已恢复 {} 个配置文件：", files.len());
    for file in files {
        println!("  {file}");
    }
    Ok(())
}

fn option(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|index| args.get(index + 1))
        .cloned()
}

fn required(args: &[String], name: &str) -> Result<String> {
    option(args, name).ok_or_else(|| anyhow::anyhow!("缺少参数 {name}"))
}

fn has_flag(args: &[String], name: &str) -> bool {
    args.iter().any(|arg| arg == name)
}

fn print_help() {
    println!(
        "JunoHub 本地客户端连接器\n\n\
         用法：\n  \
         junohub-connect setup --pair-url <JunoHub 一次性接入链接>\n  \
         junohub-connect apply --pair-url <JunoHub 一次性接入链接>\n  \
         junohub-connect apply --client <claude|codex|gemini|all> --base-url <URL> --model <别名> --api-key-stdin\n  \
         junohub-connect inspect [--client <claude|codex|gemini|all>]\n  \
         junohub-connect serve [--port <端口>]\n  \
         junohub-connect install --client <claude|codex|gemini|all>\n  \
         junohub-connect update --client <claude|codex|gemini|all>\n  \
         junohub-connect install-handler\n  \
         junohub-connect restore --backup <备份编号>\n\n\
         setup 会检测并安装缺失的客户端，然后写入接入配置。\n\
         访问密钥默认从 JUNOHUB_API_KEY 读取；使用 --api-key-stdin 可从标准输入读取。"
    );
}

#[cfg(test)]
mod management_tests {
    use super::*;

    #[test]
    fn historical_active_status_is_not_proof_of_current_configuration() {
        let home = tempfile::tempdir().unwrap();
        let file = home.path().join(".claude/settings.json");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(
            &file,
            r#"{"env":{"ANTHROPIC_BASE_URL":"https://first.test"}}"#,
        )
        .unwrap();
        record_active_provider(home.path(), "p", "Provider", "", &[Client::Claude]).unwrap();
        assert!(verified_active_providers(home.path()).contains_key("claude"));
        fs::write(
            &file,
            r#"{"env":{"ANTHROPIC_BASE_URL":"http://127.0.0.1:15721"}}"#,
        )
        .unwrap();
        assert!(!verified_active_providers(home.path()).contains_key("claude"));
        assert!(read_management_state(home.path())
            .active_providers
            .contains_key("claude"));
        fs::remove_file(&file).unwrap();
        assert!(verified_active_providers(home.path()).is_empty());
        let mut old = read_management_state(home.path());
        old.active_providers
            .get_mut("claude")
            .unwrap()
            .config_fingerprint = None;
        fs::write(
            management_state_path(home.path()),
            serde_json::to_vec(&old).unwrap(),
        )
        .unwrap();
        assert!(verified_active_providers(home.path()).is_empty());
    }
    #[test]
    fn records_active_provider_per_managed_client() {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir_all(home.path().join(".claude")).unwrap();
        fs::write(home.path().join(".claude/settings.json"), "{}").unwrap();
        fs::create_dir_all(home.path().join(".codex")).unwrap();
        fs::write(home.path().join(".codex/config.toml"), "model=\"test\"").unwrap();
        let state = record_active_provider(
            home.path(),
            "provider-1",
            "Team Gateway",
            "backup-1",
            &[Client::Claude, Client::Codex],
        )
        .unwrap();
        assert_eq!(state["claude"].id, "provider-1");
        assert_eq!(state["codex"].name, "Team Gateway");
        assert!(!state.contains_key("gemini"));
        assert_eq!(read_management_state(home.path()).active_providers, state);
    }
}
