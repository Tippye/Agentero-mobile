//! Application assembly: plugins, managed state, setup, and event wiring.

#[cfg(any(test, feature = "bindings-export"))]
pub(crate) mod bindings_test;
// `pub(crate)`: the mirror-shape anti-drift tests live in the owning modules
// of the private payload structs and reference the mirrors from here. Test
// builds only — the module does not exist otherwise.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod account;
pub(crate) mod command_util;
#[cfg(any(test, feature = "bindings-export"))]
pub(crate) mod events_contract;
mod handlers;
mod logging;
pub mod menu;
mod mobile_connectivity;
#[cfg(any(target_os = "ios", all(test, unix)))]
mod mobile_deep_links;
#[cfg(any(target_os = "ios", all(test, unix)))]
#[cfg_attr(test, allow(dead_code))]
mod mobile_documents;
#[cfg(any(target_os = "ios", test))]
mod mobile_webview;
pub(crate) mod remote_agent;
pub mod vault_session;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod workspace_cli;

#[cfg(desktop)]
pub mod finder_service;
pub use crate::features::open_request;
#[cfg(desktop)]
pub mod open_with;
#[cfg(desktop)]
pub mod terminal;
#[cfg(desktop)]
pub mod window;

#[cfg(desktop)]
use crate::features::agent::{AgentRegistry, AgentRunController};
use crate::features::markdown::wiki::WikiIndexState;
use crate::features::system::settings::AppSettingsStore;
use crate::features::vault::rename::ExternalRenameRepairStore;
#[cfg(desktop)]
use crate::features::vault::watcher::FsWatchController;
#[cfg(desktop)]
use crate::integration::connector::ConnectorController;
#[cfg(desktop)]
use crate::integration::mcp::tunnel::McpTunnelController;
#[cfg(desktop)]
use crate::integration::mcp::McpController;
#[cfg(desktop)]
use crate::integration::remote::RemoteRegistry;
// Used on every platform: the exit handler shuts down the agent warm pool.
use std::sync::Arc;
#[cfg(desktop)]
use tauri::Emitter;
use tauri::Manager;

fn manage_state(app: &tauri::AppHandle) -> Result<(), crate::core::error::AppError> {
    let settings_store = AppSettingsStore::load();
    let layout_backend = settings_store.layout_backend();
    let import_concurrency = settings_store.batch_import_concurrency();
    let translate_concurrency = settings_store.auto_translate_import_concurrency();
    app.manage(settings_store);
    #[cfg(desktop)]
    let remote_agents = crate::features::remote_agent::RemoteAgentService::default();
    #[cfg(mobile)]
    let remote_agents = crate::features::remote_agent::RemoteAgentService::with_journal(Arc::new(
        crate::integration::remote_agent_journal::SqliteOperationJournal::new(
            app.path()
                .app_data_dir()
                .map_err(|error| crate::core::error::AppError::message(error.to_string()))?
                .join("remote-agent"),
        ),
    ));
    let dsh_remote =
        Arc::new(crate::integration::dsh_remote::provider::DshRemoteProvider::default());
    remote_agents.register(dsh_remote.clone())?;
    app.manage(dsh_remote);
    app.manage(remote_agents);
    let credentials = crate::integration::credentials::CredentialStore::system(format!(
        "{}.credentials.v2",
        app.config().identifier
    ));
    app.manage(remote_agent::RemoteAgentAccounts::with_store(
        credentials.clone(),
    ));
    app.manage(Arc::new(remote_agent::RemoteAgentOAuthAttempts::default()));
    // All native secret consumers share the same backend health and mutation lane.
    app.manage(
        crate::integration::bridge::BridgeClientController::with_credentials(credentials.clone()),
    );
    app.manage(credentials);
    app.manage(
        crate::features::jobs::JobCenter::with_layout_backend(&layout_backend)
            .with_import_concurrency(import_concurrency)
            .with_layout_translate_concurrency(translate_concurrency),
    );
    app.manage(crate::features::paper::catalog::CapsCache::new());
    app.manage(crate::app::vault_session::VaultHolders::new());
    app.manage(WikiIndexState::new());
    app.manage(crate::features::vault::doctor::DoctorDirtyPathsState::default());
    app.manage(ExternalRenameRepairStore::new());
    app.manage(crate::integration::sync::SyncService::default());
    app.manage(crate::integration::sync::credentials::SyncCredentialService::default());
    #[cfg(target_os = "ios")]
    app.manage(
        crate::features::mobile_background::scene::ForegroundScenes::new(
            app.webview_windows().keys().cloned(),
        ),
    );
    crate::features::mobile_background::install(app);
    crate::features::mobile_stylus::install(app);
    app.manage(crate::features::mobile_route::MobileRouteStore::new(
        app.path()
            .app_data_dir()
            .map_err(|error| {
                crate::core::error::AppError::domain("mobile_route_unavailable", error.to_string())
            })?
            .join("mobile-routes"),
    ));
    app.manage(Arc::new(
        remote_agent::background::RemoteBackgroundMonitors::new(
            app.clone(),
            app.state::<Arc<crate::features::mobile_background::MobileBackgroundService>>()
                .inner()
                .clone(),
        ),
    ));
    mobile_connectivity::install(app);
    app.manage(crate::features::web::html_preview::HtmlPreviewGrants::default());

    #[cfg(desktop)]
    {
        // Remote vault sessions are consumed by features (agent / import /
        // trash) through inversion traits defined in the features themselves;
        // register the registry both concretely (integration commands) and as
        // each trait object (feature commands).
        let remote_registry = Arc::new(RemoteRegistry::new());
        app.manage(AgentRegistry::load());
        app.manage(AgentRunController::new());
        app.manage(Arc::new(
            crate::features::agent::session::pool::AgentWarmPool::new(),
        ));
        app.manage(Arc::new(
            crate::features::agent::session::query::AgentQueryPool::new(),
        ));
        app.manage(crate::features::agent::AgentWarmGate::new());
        app.manage(crate::features::agent::PermissionGate::new());
        app.manage(crate::features::agent::ElicitationGate::new());
        app.manage(crate::features::agent::AskUserGate::new());
        app.manage(crate::features::agent::delegate::DelegateBridge::new());
        app.manage(crate::integration::bridge::BridgeController::new());
        app.manage(FsWatchController::new());
        app.manage(crate::features::paper::ingest::IngestQueue::default());
        app.manage(Arc::new(ConnectorController::new()));
        app.manage(crate::integration::word::WordController::default());
        app.manage(Arc::new(McpController::new()));
        app.manage(Arc::new(McpTunnelController::new()));
        app.manage(remote_registry.clone());
        app.manage(remote_registry.clone() as Arc<dyn crate::features::agent::RemoteAgentHosts>);
        app.manage(
            remote_registry.clone() as Arc<dyn crate::features::paper::import::RemoteImportOps>
        );
        app.manage(
            remote_registry as Arc<dyn crate::features::vault::trash::remote_ops::RemoteTrashOps>,
        );
    }

    #[cfg(mobile)]
    {
        app.manage(
            Arc::new(crate::features::paper::import::remote_ops::LocalOnlyImports)
                as Arc<dyn crate::features::paper::import::RemoteImportOps>,
        );
    }

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        app.manage(Arc::new(crate::core::telemetry::Telemetry::new()));
        app.manage(crate::features::rtex::RtexSessions::new());
    }

    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Both ring and aws-lc-rs backends exist in the dependency tree; rustls
    // panics at connect time unless a process-wide default is installed.
    let _ = rustls::crypto::ring::default_provider().install_default();
    // Plugin callbacks can arrive before app setup. Keep these zero-I/O
    // mailboxes alive from builder creation so early deep links are retained.
    let mut builder = tauri::Builder::default()
        .manage(crate::app::open_request::PendingVaultOpen::new())
        .manage(crate::app::open_request::PendingUiRequest::new());
    #[cfg(target_os = "ios")]
    {
        builder = builder.manage(Arc::new(
            crate::features::paper::import::ios_inbox::IosInbox::default(),
        ));
    }
    if let Some(script) = agentero_core::perf::init_script() {
        builder = builder.append_invoke_initialization_script(script);
    }

    // Single-instance must be the first plugin so second-instance argv (deep
    // links on Windows/Linux) is forwarded before other plugins run.
    // Perf runs skip it so a measurement process does not attach to the
    // already-open app.
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    if !agentero_core::perf::enabled() {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            crate::app::open_request::handle_argv_urls(app, &argv);
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.show();
                let _ = win.unminimize();
                let _ = win.set_focus();
            }
        }));
    }

    #[cfg(windows)]
    {
        builder = builder.plugin(window::pinch_zoom::init());
    }

    builder = builder
        .register_asynchronous_uri_scheme_protocol("agentero-arxiv", |_ctx, request, responder| {
            crate::features::paper::discovery::proxy::arxiv::handle(request, responder);
        })
        .register_asynchronous_uri_scheme_protocol("agentero-model", |_ctx, request, responder| {
            crate::features::paper::analyze::layout::model_assets::handle_model_uri(
                request, responder,
            );
        })
        .register_asynchronous_uri_scheme_protocol(
            "agentero-coolpapers",
            |_ctx, request, responder| {
                crate::features::paper::discovery::coolpapers::proxy::handle(request, responder);
            },
        )
        .register_asynchronous_uri_scheme_protocol(
            "agentero-modelscope",
            |_ctx, request, responder| {
                crate::features::paper::discovery::proxy::modelscope::handle(request, responder);
            },
        )
        .register_asynchronous_uri_scheme_protocol(
            "agentero-huggingface",
            |_ctx, request, responder| {
                crate::features::paper::discovery::proxy::huggingface::handle(request, responder);
            },
        )
        .register_asynchronous_uri_scheme_protocol("agentero-pwc", |_ctx, request, responder| {
            crate::features::paper::discovery::proxy::pwc::handle(request, responder);
        })
        .register_asynchronous_uri_scheme_protocol("agentero-web", |_ctx, request, responder| {
            crate::features::web::proxy::handle(request, responder);
        })
        .register_asynchronous_uri_scheme_protocol("agentero-html", |ctx, request, responder| {
            crate::features::web::html_preview::handle(
                ctx.app_handle().clone(),
                request,
                responder,
            );
        })
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_dialog::init());
    #[cfg(target_os = "ios")]
    {
        builder = builder.plugin(mobile_deep_links::plugin());
    }
    #[cfg(not(target_os = "ios"))]
    {
        builder = builder.plugin(tauri_plugin_deep_link::init());
    }
    builder = builder.plugin(logging::build_log_plugin().build());

    #[cfg(mobile)]
    {
        builder = builder.plugin(tauri_plugin_agentero_mobile::init());
    }

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        builder = builder
            .plugin(account::plugin())
            .plugin(tauri_plugin_updater::Builder::new().build())
            .plugin(tauri_plugin_process::init())
            // Only the main window remembers geometry; settings / feature /
            // doc popouts keep their own fixed sizes.
            .plugin(
                tauri_plugin_window_state::Builder::default()
                    .with_state_flags(
                        tauri_plugin_window_state::StateFlags::SIZE
                            | tauri_plugin_window_state::StateFlags::POSITION
                            | tauri_plugin_window_state::StateFlags::MAXIMIZED,
                    )
                    .with_filter(|label| label == "main")
                    .build(),
            );
    }

    builder = handlers::attach_handlers(builder);

    #[cfg(target_os = "ios")]
    {
        builder = mobile_webview::attach(builder);
    }

    #[cfg(desktop)]
    {
        builder = builder.on_page_load(|webview, payload| {
            if matches!(payload.event(), tauri::webview::PageLoadEvent::Finished) {
                // The HTML boot shell is ready now; reveal it while React hydrates.
                // On Linux/GTK, calling show() on an already-visible window can
                // corrupt the native titlebar hit-test (buttons become dead until
                // a resize/double-click). Only show if currently hidden.
                let win = webview.window();
                if !win.is_visible().unwrap_or(false) {
                    let _ = win.show();
                }
            }
        });
    }

    builder = builder.setup(|app| {
        // iOS already uses its private Library roots; retain them so existing
        // paired profiles, settings, usage and feed databases keep their paths.
        #[cfg(target_os = "android")]
        crate::core::paths::initialize_app_directories(crate::core::paths::AppDirectories {
            config: app.path().app_config_dir()?,
            cache: app.path().app_cache_dir()?,
            data: app.path().app_data_dir()?,
        })?;
        vault_session::fs_scope::protect_hosted_storage(app.handle())?;
        manage_state(app.handle())?;
        // Bundled ACP adapter tier (offline fallback): record the packaged
        // adapters root before any catalog scan / spawn path can consult it.
        #[cfg(desktop)]
        {
            crate::features::agent::registry::bundled::init(app.handle());
            app.state::<Arc<crate::features::agent::delegate::DelegateBridge>>()
                .start(app.handle().clone());
        }
        // JobCenter runner registry: business domains own their executors and
        // register them here at assembly time (P2 runner-registry refactor).
        // JobCenter itself lives in features/jobs, alongside the domains it
        // schedules.
        {
            let center = app.state::<crate::features::jobs::JobCenter>();
            crate::features::paper::analyze::refs::register_job_runners(&center);
            crate::features::paper::import::job_runners::register_job_runners(&center);
            crate::features::paper::analyze::layout::model_assets::register_job_runners(&center);
            #[cfg(desktop)]
            crate::features::compile::register_job_runners(&center);
            #[cfg(not(any(target_os = "ios", target_os = "android")))]
            crate::features::jev::register_job_runners(&center);
            let handle = app.handle().clone();
            center.set_layout_backend_source(move || {
                handle.state::<AppSettingsStore>().layout_backend()
            });
            // Deep agentero-core pollers (pdf parse, asset downloads, citing
            // scans) check cancellation through the JobCenter task-id registry.
            crate::core::cancel::install_cancel_probe(crate::features::jobs::is_task_cancelled);
        }
        // Shared System One provider for the smart-highlight stream.
        app.manage(crate::features::decision::build_provider(
            app.handle().clone(),
        ));
        let settings_store = app.state::<AppSettingsStore>();
        #[cfg(desktop)]
        let agents = app.state::<AgentRegistry>();
        let settings = settings_store
            .get()
            .map(|result| result.settings)
            .unwrap_or_default();
        #[cfg(desktop)]
        let mut settings = settings;
        // Migrate the old Agent-only proxy once into the shared network setting.
        #[cfg(desktop)]
        if !settings.network_proxy_enabled {
            if let Ok((enabled, url)) = agents.proxy_settings() {
                if enabled {
                    settings.network_proxy_enabled = true;
                    settings.network_proxy_url = url;
                    let _ = settings_store.set(settings.clone());
                }
            }
        }
        // Settings-change reactions: domains subscribe at assembly time, so
        // the settings feature stays schema-agnostic with no edges into
        // agent/connector/import/jobs (P2-18 settings-layer refactor; mirrors
        // the JobCenter runner-registry pattern). Registered after the
        // one-shot proxy migration above so that migration `set` does not
        // fire reactions this boot sequence applies manually below.
        {
            #[cfg(desktop)]
            {
                let handle = app.handle().clone();
                settings_store.subscribe(move |s| {
                    let _ = handle
                        .state::<AgentRegistry>()
                        .set_proxy(s.network_proxy_enabled, s.network_proxy_url.clone());
                });
            }
            #[cfg(not(any(target_os = "ios", target_os = "android")))]
            {
                let handle = app.handle().clone();
                settings_store.subscribe(move |_s| {
                    crate::features::paper::import::refresh_parser_config(
                        &handle.state::<AppSettingsStore>(),
                    );
                });
            }
            #[cfg(desktop)]
            {
                let handle = app.handle().clone();
                settings_store.subscribe(move |s| {
                    let ctrl = Arc::clone(handle.state::<Arc<ConnectorController>>().inner());
                    let port = s.connector_port;
                    // `set_port` is async (it may rebind the listener); the
                    // result was always discarded and the controller emits its
                    // own status event, so run it on the runtime.
                    tauri::async_runtime::spawn(async move {
                        let _ = ctrl.set_port(port).await;
                    });
                });
            }
            #[cfg(desktop)]
            {
                let handle = app.handle().clone();
                settings_store.subscribe(move |s| {
                    let ctrl = Arc::clone(handle.state::<Arc<McpController>>().inner());
                    let port = s.mcp_port;
                    let translator =
                        crate::features::paper::import::DEFAULT_TRANSLATOR_BASE_URL.to_string();
                    let note_mode = s.paper_note_mode.clone();
                    ctrl.set_translator_url(Some(translator));
                    ctrl.set_paper_note_mode(note_mode);
                    // `set_port` stops and restarts the listener even when the
                    // port is unchanged; doing that on every settings change
                    // (e.g. editing the tunnel ID) would briefly hide the MCP
                    // status dot and disable the tunnel Start button.
                    if port != ctrl.port() {
                        tauri::async_runtime::spawn(async move {
                            let _ = ctrl.set_port(port).await;
                        });
                    }
                });
            }
            #[cfg(desktop)]
            {
                let handle = app.handle().clone();
                settings_store.subscribe(move |s| {
                    let tunnel = Arc::clone(handle.state::<Arc<McpTunnelController>>().inner());
                    let mcp_url = format!("http://127.0.0.1:{}/mcp", s.mcp_port);
                    tauri::async_runtime::spawn(async move {
                        let _ = tunnel.mcp_url_changed(mcp_url).await;
                    });
                });
            }
            {
                let handle = app.handle().clone();
                settings_store.subscribe(move |s| {
                    let center = handle
                        .state::<crate::features::jobs::JobCenter>()
                        .inner()
                        .clone();
                    let backend = s.layout.backend.clone();
                    let import_cap = s.batch_import_concurrency.max(1) as usize;
                    // Out-of-range background-translation caps fall back to 1
                    // instead of parking papers forever.
                    let translate_cap = s.translate.auto_translate_import_concurrency.clamp(
                        1,
                        crate::features::system::settings::MAX_AUTO_TRANSLATE_IMPORT_CONCURRENCY,
                    ) as usize;
                    let app = handle.clone();
                    tauri::async_runtime::spawn(async move {
                        center.apply_layout_backend(&backend).await;
                        center.apply_import_concurrency(import_cap).await;
                        center
                            .apply_layout_translate_concurrency(translate_cap)
                            .await;
                        center.drain_and_spawn(&app).await;
                    });
                });
            }
        }
        crate::core::http::configure_proxy(
            settings.network_proxy_enabled,
            &settings.network_proxy_url,
        )?;
        // A page fetch or PDF download that hits an interactive verification
        // wall (OpenReview Turnstile, Cloudflare) asks this hook to open the
        // blocked URL in a window (#1157). Mobile has no such window, and the
        // CLI has no Host at all; both report the wall as a readable error.
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        crate::features::system::settings::commands::install_web_verification_hook(
            app.handle().clone(),
        );
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        crate::features::paper::import::refresh_parser_config(&settings_store);
        #[cfg(desktop)]
        let _ = agents.set_proxy(
            settings.network_proxy_enabled,
            settings.network_proxy_url.clone(),
        );
        // Prefetch PP-DocLayoutV3 into XDG as a JobCenter `modelDownload` job;
        // the tasks-panel projection shows progress, concurrent triggers dedupe.
        // Perf runs skip it so a startup measurement is not mixed with a download.
        #[cfg(desktop)]
        if !agentero_core::perf::enabled() {
            crate::features::paper::analyze::layout::model_assets::spawn_background_download(
                app.handle().clone(),
            );
        }
        // Native menu is macOS-only; the renderer re-syncs the locale on mount.
        #[cfg(target_os = "macos")]
        {
            menu::setup(app.handle())?;
            if let Some(main_win) = app.get_webview_window("main") {
                window::commands::enable_window_tiling(&main_win);
            }
        }
        // Ensure registry is loaded early.
        #[cfg(desktop)]
        let _ = app.state::<AgentRegistry>();
        let _ = app.state::<WikiIndexState>();
        let _ = app.state::<crate::features::vault::doctor::DoctorDirtyPathsState>();
        let _ = app.state::<ExternalRenameRepairStore>();
        #[cfg(desktop)]
        {
            let connector = app.state::<Arc<ConnectorController>>();
            connector.set_app_handle(app.handle().clone());
            let remote = app.state::<Arc<RemoteRegistry>>();
            connector.set_remote_registry(Arc::clone(&remote));
        }
        #[cfg(desktop)]
        {
            let mcp = app.state::<Arc<McpController>>();
            crate::integration::mcp::attach_app(&mcp, app.handle().clone());
            mcp.set_translator_url(Some(
                crate::features::paper::import::DEFAULT_TRANSLATOR_BASE_URL.to_string(),
            ));
            mcp.set_paper_note_mode(settings.paper_note_mode.clone());
        }
        #[cfg(desktop)]
        {
            let tunnel = app.state::<Arc<McpTunnelController>>();
            tunnel.set_app_handle(app.handle().clone());
            tunnel.schedule_startup_sweep();
        }
        log::info!(
            target: "agentero::op",
            "op start app_ready debug={}",
            cfg!(debug_assertions)
        );

        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        if !agentero_core::perf::enabled() {
            let telemetry = Arc::clone(
                app.state::<Arc<crate::core::telemetry::Telemetry>>()
                    .inner(),
            );
            // Registry-only read (no PATH probing), safe on the setup path.
            let agent_summary = agents.telemetry_summary();
            // Assemble the start context here so core::telemetry stays free
            // of upward edges into features (settings / agent).
            let start_ctx = crate::core::telemetry::TelemetryStartContext {
                telemetry_enabled: settings.telemetry_enabled,
                locale: settings.locale.clone(),
                installed_agents: agent_summary.templates,
                custom_agent_count: agent_summary.custom_count,
            };
            tauri::async_runtime::spawn_blocking(move || {
                telemetry.start(start_ctx);
            });
        }

        // Desktop deep links: cold start + runtime open URLs.
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        {
            use tauri_plugin_deep_link::DeepLinkExt;
            // Linux / Windows dev: associate schemes with the current executable.
            #[cfg(any(target_os = "linux", all(debug_assertions, windows)))]
            {
                if let Err(e) = app.deep_link().register_all() {
                    log::warn!(target: "agentero::op", "deep_link register_all failed: {e}");
                }
            }
            if let Ok(Some(urls)) = app.deep_link().get_current() {
                let list: Vec<String> = urls.into_iter().map(|u| u.to_string()).collect();
                crate::app::open_request::handle_deep_link_urls(app.handle(), &list);
            }
            // Dev / direct spawn: CLI may pass agentero://… as argv when the
            // OS scheme is not registered (common with `tauri dev`).
            let argv: Vec<String> = std::env::args().collect();
            crate::app::open_request::handle_argv_urls(app.handle(), &argv);
            // Consume a request file left by `agentero open` before we listened.
            if let Some(path) = crate::app::open_request::take_cli_open_request_file() {
                let _ = crate::app::open_request::handle_open_path(app.handle(), &path);
            }
            if let Some(req) = crate::app::open_request::take_cli_ui_request_file() {
                let _ = crate::app::open_request::handle_ui_request(app.handle(), req);
            }
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            if let Err(error) = workspace_cli::start(app.handle().clone()) {
                log::warn!(target:"agentero::cli","desktop command channel unavailable: {error}");
            }
            crate::app::open_request::spawn_cli_open_request_watcher(app.handle().clone());
            let handle = app.handle().clone();
            app.deep_link().on_open_url(move |event| {
                let list: Vec<String> = event.urls().into_iter().map(|u| u.to_string()).collect();
                crate::app::open_request::handle_deep_link_urls(&handle, &list);
            });
        }

        // Finder "Open with Agentero" Quick Action: default-installed and kept
        // current on macOS (opt-out remembered via a config marker).
        #[cfg(target_os = "macos")]
        {
            tauri::async_runtime::spawn_blocking(|| {
                crate::app::finder_service::ensure_installed();
            });
        }

        // Desktop watches files; mobile uses foreground lifecycle and save hints.
        app.state::<crate::integration::sync::SyncService>()
            .start_all(app.handle());

        Ok(())
    });

    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        builder = builder.on_menu_event(|app, event| {
            let id = event.id().as_ref();
            if id == "close_tab_or_window" {
                // Cmd+W is app-global on macOS. When Settings is focused, close
                // that native window instead of forwarding the command to the
                // main renderer, where it would close the active document tab.
                if let Some(settings) =
                    app.get_webview_window(crate::app::window::commands::SETTINGS_WINDOW_LABEL)
                {
                    if settings.is_focused().unwrap_or(false) {
                        let _ = settings.close();
                        return;
                    }
                }
            }
            if id == "new_window" {
                // `window_new` is async so webview creation never runs inside this
                // main-thread menu callback (see its doc comment).
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = crate::app::window::commands::window_new(app).await {
                        log::error!(target: "agentero::op", "op end window_new ok=false error={e}");
                    }
                });
                return;
            }
            // Remaining menu ids are renderer actions: broadcast one
            // `menu:invoked` event with the action id.
            let _ = app.emit(
                crate::app::menu::MENU_INVOKED_EVENT,
                serde_json::json!({ "action": id }),
            );
        });
    }

    #[cfg(desktop)]
    {
        builder = builder.on_window_event(|window, event| {
            if matches!(event, tauri::WindowEvent::Destroyed) {
                #[cfg(target_os = "macos")]
                menu::release_window(window.app_handle(), window.label());
                window.state::<FsWatchController>().stop(window.label());
                // The window is gone, so it must not keep pinning any vault's
                // Host resources (see `VaultHolders`). The global release
                // sequence is async; spawn it so this synchronous event
                // callback never blocks the event loop.
                crate::features::agent::hosted_turn::revoke_window(window.label(), None);
                let drained = window
                    .state::<vault_session::VaultHolders>()
                    .drain_window(window.label());
                if !drained.is_empty() {
                    let app = window.app_handle().clone();
                    tauri::async_runtime::spawn(async move {
                        for vault in drained {
                            vault_session::lifecycle::release_vault_resources(
                                &app,
                                std::path::Path::new(&vault),
                            )
                            .await;
                        }
                    });
                }
                if window.label() == crate::app::window::commands::SETTINGS_WINDOW_LABEL {
                    crate::app::window::commands::emit_window_closed(
                        window.app_handle(),
                        "settings",
                        None,
                    );
                }
                if let Some(view) =
                    crate::app::window::commands::feature_view_from_label(window.label())
                {
                    crate::app::window::commands::emit_window_closed(
                        window.app_handle(),
                        "feature",
                        Some(view),
                    );
                }
            }
        });
    }

    #[cfg(mobile)]
    {
        builder = builder.on_window_event(|window, event| {
            let service = window.state::<crate::integration::sync::SyncService>();
            let jobs = window.state::<crate::features::jobs::JobCenter>();
            #[cfg(target_os = "ios")]
            {
                let active = match event {
                    tauri::WindowEvent::Resumed | tauri::WindowEvent::Focused(true) => Some(true),
                    tauri::WindowEvent::Suspended | tauri::WindowEvent::Destroyed => Some(false),
                    _ => None,
                };
                if let Some(foreground) = active.and_then(|active| window.state::<crate::features::mobile_background::scene::ForegroundScenes>().update(window.label(), active)) {
                    service.set_foreground(window.app_handle(), foreground);
                    jobs.set_renderer_foreground(foreground);
                    let monitors = window.state::<Arc<remote_agent::background::RemoteBackgroundMonitors>>();
                    if foreground { monitors.resume(); } else {
                        monitors.suspend();
                        window.state::<Arc<crate::features::mobile_background::MobileBackgroundService>>().suspend();
                    }
                }
            }
            #[cfg(target_os = "android")]
            match event {
                tauri::WindowEvent::Suspended => {
                    service.set_foreground(window.app_handle(), false);
                    jobs.set_renderer_foreground(false);
                }
                tauri::WindowEvent::Resumed | tauri::WindowEvent::Focused(true) => {
                    service.set_foreground(window.app_handle(), true);
                    jobs.set_renderer_foreground(true);
                }
                _ => {}
            }
        });
    }

    #[cfg_attr(not(target_os = "linux"), allow(unused_mut))]
    let mut context = tauri::generate_context!();
    // WebKitGTK can omit dropped File objects. Keep GTK's native file-path
    // events enabled so the frontend's existing OS-drop handlers can import.
    // Unlike WebView2, GTK still forwards in-app HTML5 drags.
    #[cfg(target_os = "linux")]
    for window in &mut context.config_mut().app.windows {
        window.drag_drop_enabled = true;
    }

    builder
        .build(context)
        .expect("error while building tauri application")
        .run(|app, event| {
            let _ = &app;
            #[cfg(target_os = "ios")]
            if let tauri::RunEvent::Opened { urls } = &event {
                mobile_documents::receive_opened(app, urls);
            }
            if matches!(
                event,
                tauri::RunEvent::Exit | tauri::RunEvent::ExitRequested { .. }
            ) {
                mobile_connectivity::shutdown(app);
                app.state::<Arc<remote_agent::background::RemoteBackgroundMonitors>>()
                    .shutdown();
                let background = app
                    .state::<Arc<crate::features::mobile_background::MobileBackgroundService>>()
                    .inner()
                    .clone();
                tauri::async_runtime::spawn(async move {
                    let _ = background.shutdown().await;
                });
                #[cfg(desktop)]
                {
                    app.state::<Arc<crate::features::agent::session::pool::AgentWarmPool>>()
                        .shutdown();
                    app.state::<Arc<crate::features::agent::session::query::AgentQueryPool>>()
                        .shutdown();
                    app.state::<Arc<crate::features::agent::delegate::DelegateBridge>>()
                        .shutdown();
                }
                let _ = app
                    .state::<crate::integration::bridge::BridgeClientController>()
                    .disconnect();
                #[cfg(desktop)]
                app.state::<Arc<ConnectorController>>().stop();
                // The MCP HTTP listener and the ChatGPT tunnel child are both
                // desktop-only; stop them here so port 8765/8080 are released
                // when the app quits. This matches Connector/Bridge behavior.
                #[cfg(desktop)]
                {
                    app.state::<Arc<McpController>>().stop_server();
                    app.state::<Arc<McpTunnelController>>().stop();
                }
                // Close the mobile relay rather than letting the socket die with
                // the process, so paired clients see a clean shutdown. `stop`
                // takes the runtime, so the second call in this arm is a no-op.
                #[cfg(desktop)]
                let _ = app
                    .state::<crate::integration::bridge::BridgeController>()
                    .stop();
                #[cfg(not(any(target_os = "android", target_os = "ios")))]
                app.state::<Arc<crate::core::telemetry::Telemetry>>()
                    .shutdown();
                #[cfg(not(any(target_os = "android", target_os = "ios")))]
                app.state::<crate::features::rtex::RtexSessions>()
                    .stop_all();
            }
            // Best-effort final push (bounded); only on Exit so it runs once.
            #[cfg(desktop)]
            if matches!(event, tauri::RunEvent::Exit) {
                app.state::<crate::integration::sync::SyncService>()
                    .flush_on_exit(app);
            }
        });
}
