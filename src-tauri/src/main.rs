#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod agent;
mod agent_parse;
mod agent_prompts;
mod agent_stream;
mod ai_job;
mod baseline;
mod config;
mod core;
mod crypto;
mod fs;
mod intercept;
mod known_hosts;
mod monitor;
mod recorder;
mod restore;
mod safety;
mod skills;
mod snapshot;

mod ssh;
mod task_exec;

use tauri::{LogicalSize, Manager, PhysicalPosition, PhysicalSize, RunEvent, WindowEvent};

/// 窗口逻辑尺寸下限：低于此值视为最小化/异常状态遗留的图标尺寸，不入库、不恢复
/// （P28 的 ±20000 哨兵只守了位置；实测最小化时 Windows 报 ~200x35 物理 ≈ 159x27 逻辑，
/// 退出落盘后下次启动就是不可用的小窗，且会被 rdp_connect 误用作桌面分辨率）
const MIN_WINDOW_LOGICAL_W: u32 = 320;
const MIN_WINDOW_LOGICAL_H: u32 = 240;

/// 物理尺寸 → 合理逻辑尺寸；低于下限返回 None（视为最小化脏值）
fn sane_logical_size(size: PhysicalSize<u32>, sf: f64) -> Option<(u32, u32)> {
    let logical = size.to_logical::<f64>(sf);
    if logical.width >= MIN_WINDOW_LOGICAL_W as f64 && logical.height >= MIN_WINDOW_LOGICAL_H as f64
    {
        Some((logical.width.round() as u32, logical.height.round() as u32))
    } else {
        None
    }
}

fn main() {
    // 加载配置（命令行 > ./config.yaml > ../config.yaml > ~/.config/helm/config.yaml）
    let (config_path, config) = match config::load_config_with_path(None) {
        Ok(pair) => pair,
        Err(_) => {
            // dev 模式下 CWD 是 src-tauri，向上找项目根 config.yaml
            let parent = std::path::PathBuf::from("../config.yaml");
            match config::load_config_with_path(parent.to_str()) {
                Ok(pair) => pair,
                Err(e) => {
                    eprintln!("配置加载失败: {}", e);
                    (parent, config::HelmConfig::default())
                }
            }
        }
    };

    let app = tauri::Builder::default()
        .manage(core::CoreState::new(config_path, config))
        .setup(|app| {
            let state = app.state::<core::CoreState>();
            core::spawn_output_poller(app.handle().clone(), state.ssh.clone());
            monitor::spawn_sysmon_poller(app.handle().clone(), state.ssh.clone());

            // P26：恢复上次窗口尺寸与位置（未记录位置时保持系统默认/居中）
            if let Some(win) = app.get_webview_window("main") {
                let ui = state.config.blocking_lock().ui.clone().unwrap_or_default();
                // 尺寸脏值自愈：历史版本曾把最小化图标尺寸落盘，低于下限时回退默认值
                let (w, h) = if ui.window_width >= MIN_WINDOW_LOGICAL_W
                    && ui.window_height >= MIN_WINDOW_LOGICAL_H
                {
                    (ui.window_width as f64, ui.window_height as f64)
                } else {
                    let fallback = config::UiConfig::default();
                    (fallback.window_width as f64, fallback.window_height as f64)
                };
                let _ = win.set_size(LogicalSize::new(w, h));
                if let (Some(x), Some(y)) = (ui.window_x, ui.window_y) {
                    // 跳过离屏哨兵坐标（最小化遗留的 -32000 等），恢复默认位置
                    if x.abs() >= 20000 || y.abs() >= 20000 {
                        let _ = win.set_position(PhysicalPosition::new(0, 0));
                    } else {
                        let _ = win.set_position(PhysicalPosition::new(x, y));
                    }
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| match event {
            // P26：窗口移动时记录位置到内存（退出时统一落盘）。
            // 最小化时 Windows 会把窗口移到 (-32000,-32000) 这类离屏哨兵值，
            // 跳过离谱坐标，避免把离屏位置落盘导致下次启动恢复不到屏幕内。
            WindowEvent::Moved(pos)
                if pos.x.abs() < 20000 && pos.y.abs() < 20000 =>
            {
                if let Some(state) = window.try_state::<core::CoreState>() {
                    let mut cfg = state.config.blocking_lock();
                    if let Some(ui) = cfg.ui.as_mut() {
                        ui.window_x = Some(pos.x);
                        ui.window_y = Some(pos.y);
                    }
                }
            }
            // 尺寸同样即时记录（带下限守卫）：最小化也会触发 Resized 但报图标尺寸被拒，
            // 还原/手动缩放后恢复记录 —— 保证「缩放后又最小化再退出」不丢最后一次真实尺寸。
            // 口径 = inner（客户区）：tao 的 Resized 载荷即客户区物理尺寸，
            // 与 set_size(LogicalSize)（→ set_inner_size）及 JS innerWidth/innerHeight 一致，
            // 避免 outer/inner 混用导致每次开关窗口长大后一圈边框
            WindowEvent::Resized(size) => {
                let sf = window.scale_factor().unwrap_or(1.0);
                if let Some((w, h)) = sane_logical_size(*size, sf) {
                    if let Some(state) = window.try_state::<core::CoreState>() {
                        let mut cfg = state.config.blocking_lock();
                        if let Some(ui) = cfg.ui.as_mut() {
                            ui.window_width = w;
                            ui.window_height = h;
                        }
                    }
                }
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            core::list_sessions,
            core::session_status,
            core::add_session,
            core::update_session,
            core::delete_session,
            core::connect_session,
            core::disconnect_session,
            core::set_active,
            core::clear_active,
            core::rdp_connect,
            core::open_external,
            core::send_input,
            core::send_active_input,
            core::resize_sessions,
            core::ai_submit,
            core::ai_control,
            core::ai_stop,
            core::ai_clear_history,
            core::ai_set_mode,
            core::ai_mode,
            core::get_ai_config,
            core::update_ai_config,
            core::test_ai_connection,
            core::ai_list_models,
            core::get_ui_config,
            core::update_ui_config,
            core::fs_list_dir,
            core::fs_current_dir,
            core::fs_mkdir,
            core::fs_rename,
            core::fs_remove,
            core::fs_upload,
            core::fs_download,
            core::fs_read_file,
            core::toggle_fullscreen,
            core::ai_conv_read,
            core::ai_conv_clear,
            core::history_read,
            core::history_export,
            core::recording_info,
            core::zmodem_save,
            core::forget_host_key,
            core::host_key_fingerprint,
            core::docker_ps,
        core::snapshot_probe,
        core::snapshot_pack,
        core::snapshot_list,
        core::snapshot_chains,
        core::snapshot_remove,
        core::snapshot_prune,
        core::snapshot_members,
        core::snapshot_fetch_file,
        core::snapshot_restore_file,
        core::snapshot_set_intercept,
        core::snapshot_get_intercept,
        core::intercept_resolve,
        core::intercept_pending,
        core::baseline_set_enabled,
        core::baseline_status,
        core::baseline_observe,
        core::snapshot_dry_run,
        core::snapshot_restore,
        core::snapshot_discard_staged,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    // P26：退出时把窗口位置/尺寸写回配置，下次启动恢复
    app.run(|app_handle, event| {
        if let RunEvent::Exit = event {
            if let Some(state) = app_handle.try_state::<core::CoreState>() {
                let mut cfg = state.config.blocking_lock();
                if let Some(ui) = cfg.ui.as_mut() {
                    if let Some(win) = app_handle.get_webview_window("main") {
                        // 位置：移动事件可能已记录，这里兜底取当前值（跳过最小化离屏哨兵）
                        if let Ok(pos) = win.outer_position() {
                            if pos.x.abs() < 20000 && pos.y.abs() < 20000 {
                                ui.window_x = Some(pos.x);
                                ui.window_y = Some(pos.y);
                            }
                        }
                        // 尺寸：记录当前逻辑尺寸（与 set_size(LogicalSize) 同口径 = inner 客户区）；
                        // 最小化状态退出时报的是图标尺寸，经下限守卫拒绝（保留 Resized 已记录值）
                        let sf = win.scale_factor().unwrap_or(1.0);
                        if let Ok(size) = win.inner_size() {
                            if let Some((w, h)) = sane_logical_size(size, sf) {
                                ui.window_width = w;
                                ui.window_height = h;
                            }
                        }
                    }
                }
                let _ = config::save_config(&state.config_path, &cfg);
            }
        }
    });
}
