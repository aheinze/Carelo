use crate::store::{window_dimensions, AppStoreState};
use tauri::{App, AppHandle, LogicalSize, Manager, Runtime, WebviewWindow, WindowEvent};

const MAIN_WINDOW_LABEL: &str = "main";

pub fn restore_main_window_state<R: Runtime>(app: &App<R>) {
    let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) else {
        return;
    };

    if let Some(store) = app.try_state::<AppStoreState>() {
        if let Ok(Some(dimensions)) = store.window_dimensions() {
            let _ = window.set_size(LogicalSize::new(dimensions.width, dimensions.height));
        }
    }

    register_window_dimension_persistence(&window, app.handle().clone());
}

pub fn save_main_window_dimensions<R: Runtime>(app: &AppHandle<R>) {
    let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) else {
        return;
    };

    if let Some(store) = app.try_state::<AppStoreState>() {
        save_window_dimensions(&window, &store);
    }
}

fn register_window_dimension_persistence<R: Runtime>(window: &WebviewWindow<R>, app: AppHandle<R>) {
    window.on_window_event(move |event| {
        if matches!(event, WindowEvent::CloseRequested { .. }) {
            save_main_window_dimensions(&app);
        }
    });
}

fn save_window_dimensions<R: Runtime>(window: &WebviewWindow<R>, store: &AppStoreState) {
    let Ok(size) = window.inner_size() else {
        return;
    };
    let scale_factor = window.scale_factor().unwrap_or(1.0);

    if !scale_factor.is_finite() || scale_factor <= 0.0 {
        return;
    }

    let logical_size = size.to_logical::<f64>(scale_factor);
    let _ =
        store.save_window_dimensions(window_dimensions(logical_size.width, logical_size.height));
}
