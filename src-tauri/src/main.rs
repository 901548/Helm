#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod agent;
mod ai_job;
mod config;
mod core;
mod crypto;
mod fs;
mod known_hosts;
mod monitor;
mod safety;
mod ssh;
mod task_exec;

use tauri::{LogicalSize, Manager, PhysicalPosition, RunEvent, WindowEvent};

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
                let _ = win.set_size(LogicalSize::new(
                    ui.window_width as f64,
                    ui.window_height as f64,
                ));
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
        .on_window_event(|window, event| {
            // P26：窗口移动时记录位置到内存（退出时统一落盘）。
            // 最小化时 Windows 会把窗口移到 (-32000,-32000) 这类离屏哨兵值，
            // 跳过离谱坐标，避免把离屏位置落盘导致下次启动恢复不到屏幕内。
            if let WindowEvent::Moved(pos) = event {
                if pos.x.abs() < 20000 && pos.y.abs() < 20000 {
                    if let Some(state) = window.try_state::<core::CoreState>() {
                        let mut cfg = state.config.blocking_lock();
                        if let Some(ui) = cfg.ui.as_mut() {
                            ui.window_x = Some(pos.x);
                            ui.window_y = Some(pos.y);
                        }
                    }
                }
            }
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
            core::forget_host_key,
            core::host_key_fingerprint,
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
                        // 尺寸：记录当前逻辑尺寸（与 update_ui_config 的 LogicalSize 一致）
                        let sf = win.scale_factor().unwrap_or(1.0);
                        if let Ok(size) = win.outer_size() {
                            let logical = size.to_logical::<f64>(sf);
                            ui.window_width = logical.width.round() as u32;
                            ui.window_height = logical.height.round() as u32;
                        }
                    }
                }
                let _ = config::save_config(&state.config_path, &cfg);
            }
        }
    });
}
