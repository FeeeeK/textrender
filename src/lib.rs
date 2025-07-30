mod logging;

use std::{
    hash::{Hash, Hasher},
    mem::transmute,
    sync::LazyLock,
    time::Duration,
};

use eldenring::{
    cs::{CSWindowImp, CSWindowType},
    position::HavokPosition,
};
use eldenring_util::{program::Program, singleton::get_instance, system::wait_for_system_init};

use crate::logging::{custom_panic_hook, setup_logging};
use crossbeam_queue::ArrayQueue;
use hudhook::{
    Hudhook, ImguiRenderLoop, RenderContext,
    imgui::{self, FontGlyphRanges, Ui},
    windows::Win32::{
        Foundation::HINSTANCE,
        System::{LibraryLoader::DisableThreadLibraryCalls, SystemServices::DLL_PROCESS_ATTACH},
    },
};
use hudhook::{hooks::dx12::ImguiDx12Hooks, imgui::Context};
use pelite::pe::Pe;
use retour::static_detour;

static TEXT_RENDER_QUEUE: LazyLock<ArrayQueue<DrawCommand>> =
    LazyLock::new(|| ArrayQueue::new(10000));

const BASE_IMGUI_FONT_SIZE_PX: f32 = 13.0;

#[derive(Debug)]
enum DrawCommand {
    Text(String, f32, f32),
    SetFontSize(f32),
    SetTextScale(f32, f32, f32),
    ResetTextScale,
}

fn u16_ptr_to_string(ptr: *const u16) -> String {
    let len = (0..)
        .take_while(|&i| unsafe { *ptr.offset(i) } != 0)
        .count();
    let slice = unsafe { std::slice::from_raw_parts(ptr, len) };

    String::from_utf16(slice).unwrap_or(String::from("?EncodingError?"))
}

// void FUN_14264ef60(CSEzDraw *param_1,FloatVector4 *param_2,wchar_t *param_3)
const TEXT_RENDER_REQUEST_RVA: u32 = 0x264ef60;
// void CS::CSEzDraw::SetFontSize(CSEzDraw *param_1,float fontSize)
const SET_FONT_SIZE_RVA: u32 = 0xbb6310;
// void CS::CSEzDraw::SetTextScale(CSEzDraw *param_1,float textPosWidthScate,float textPosHeightScate,float fontSize)
const SET_TEXT_SCALE_RVA: u32 = 0x1def10;
// void CS::CSEzDraw::ResetTextScale(CSEzDraw *param_1)
const RESET_TEXT_SCALE_RVA: u32 = 0xbb6290;
// void CS::CSEzDraw::DrawTextWithSize(CSEzDraw *param_1,FloatVector4 *param_2,float *param_3,wchar_t *param_4)
const DRAW_TEXT_WITH_SIZE_RVA: u32 = 0x264eec0;

static_detour! {
    static DrawTextRenderRequest: unsafe extern "C" fn(usize, *mut HavokPosition, *const u16) -> ();
    static SetFontSize: unsafe extern "C" fn(usize, f32) -> ();
    static SetTextScale: unsafe extern "C" fn(usize, f32, f32, f32) -> ();
    static ResetTextScale: unsafe extern "C" fn(usize) -> ();
    static DrawTextWithSize: unsafe extern "C" fn(usize, *mut HavokPosition, *mut f32, *const u16) -> ();
}

struct DebugTextRender {
    text_scale: (f32, f32),
    font_size: f32,
}
impl DebugTextRender {
    fn new() -> Self {
        Self {
            text_scale: (1.0, 1.0),
            font_size: 24.0,
        }
    }

    fn reset_size(&mut self) {
        let window_size = Self::window_size();
        let screen_size = Self::get_screen_size();
        let aspect_w = screen_size[0] / window_size[0];
        let aspect_h = screen_size[1] / window_size[1];
        self.text_scale = (aspect_w, aspect_h);
    }

    fn get_aspect_ratios() -> (f32, f32) {
        let window_size = Self::window_size();
        let screen_size = Self::get_screen_size();
        let mut aspect_w = screen_size[0] / window_size[0];
        let mut aspect_h = screen_size[1] / window_size[1];
        if aspect_w < 0.8 {
            aspect_w = 0.8;
        }
        if aspect_h < 0.8 {
            aspect_h = 0.8;
        }
        (aspect_w, aspect_h)
    }

    fn get_screen_size() -> [f32; 2] {
        if let Ok(Some(window)) = unsafe { get_instance::<CSWindowImp>() } {
            [window.screen_width as f32, window.screen_height as f32]
        } else {
            [1920.0, 1080.0]
        }
    }

    fn window_size() -> [f32; 2] {
        if let Ok(Some(window)) = unsafe { get_instance::<CSWindowImp>() } {
            match window.persistent_window_config.window_type {
                CSWindowType::Windowed => [
                    window.persistent_window_config.windowed_screen_width as f32,
                    window.persistent_window_config.windowed_screen_height as f32,
                ],
                CSWindowType::Fullscreen => [
                    window.persistent_window_config.fullscreen_width as f32,
                    window.persistent_window_config.fullscreen_height as f32,
                ],
                CSWindowType::Borderless => [
                    window.persistent_window_config.borderless_screen_width as f32,
                    window.persistent_window_config.borderless_screen_height as f32,
                ],
            }
        } else {
            [1920.0, 1080.0]
        }
    }
}

impl ImguiRenderLoop for DebugTextRender {
    fn initialize(&mut self, ctx: &mut Context, _render_context: &mut dyn RenderContext) {
        let font_data = std::fs::read("C:\\Windows\\Fonts\\msgothic.ttc")
            .expect("Failed to read font file (msgothic.ttc)");
        ctx.fonts().add_font(&[imgui::FontSource::TtfData {
            data: &font_data,
            size_pixels: BASE_IMGUI_FONT_SIZE_PX,
            config: Some(imgui::FontConfig {
                oversample_h: 3,
                oversample_v: 1,
                pixel_snap_h: true,
                glyph_ranges: FontGlyphRanges::japanese(),
                ..Default::default()
            }),
        }]);
        ctx.fonts().build_alpha8_texture();
    }

    fn render(&mut self, ui: &mut Ui) {
        // Workaround for crash on empty render queue
        ui.window("_")
            .size([1.0, 1.0], imgui::Condition::FirstUseEver)
            .position([0.0, 0.0], imgui::Condition::FirstUseEver)
            .no_decoration()
            .draw_background(false)
            .no_inputs()
            .resizable(false)
            .movable(false)
            .collapsible(false)
            .title_bar(false)
            .build(|| ui.text("."));

        while let Some(event) = TEXT_RENDER_QUEUE.pop() {
            match event {
                DrawCommand::Text(text, x, y) => {
                    tracing::debug!("Text: {} at ({}, {})", text, x, y);

                    let scaled_x = x * self.text_scale.0;
                    let scaled_y = y * self.text_scale.1;
                    // normalize the coordinates to the screen space
                    let window_size = Self::get_screen_size();
                    let scaled_x = (scaled_x % window_size[0] + window_size[0]) % window_size[0];
                    let scaled_y = (scaled_y % window_size[1] + window_size[1]) % window_size[1];

                    // Hash the coordinates and text to create a unique window name
                    let mut hasher = std::collections::hash_map::DefaultHasher::new();
                    (x as u32).hash(&mut hasher);
                    (y as u32).hash(&mut hasher);
                    (scaled_x as u32).hash(&mut hasher);
                    (scaled_y as u32).hash(&mut hasher);
                    text.hash(&mut hasher);

                    ui.window(format!("text_window_{}_{}_{}", x, y, hasher.finish()))
                        .size(Self::get_screen_size(), imgui::Condition::Always)
                        .position([scaled_x, scaled_y], imgui::Condition::Always)
                        .no_decoration()
                        .draw_background(false)
                        .no_inputs()
                        .resizable(false)
                        .movable(false)
                        .collapsible(false)
                        .title_bar(false)
                        .build(|| {
                            let font_scale_factor = self.font_size / BASE_IMGUI_FONT_SIZE_PX;
                            ui.set_window_font_scale(font_scale_factor);
                            ui.text(text);
                        });
                }
                DrawCommand::SetFontSize(scale) => {
                    tracing::debug!("Font size: {}", scale);
                    self.font_size = scale;
                }

                DrawCommand::SetTextScale(mut width_scale, mut height_scale, _font_size) => {
                    tracing::debug!(
                        "Width scale: {}, Height scale: {}, Font size: {}",
                        width_scale,
                        height_scale,
                        _font_size
                    );

                    let (aspect_w, aspect_h) = Self::get_aspect_ratios();
                    width_scale *= aspect_w;
                    height_scale *= aspect_h;

                    if width_scale != self.text_scale.0 || height_scale != self.text_scale.1 {
                        self.text_scale = (width_scale, height_scale);
                    }
                    self.font_size = _font_size;
                }
                DrawCommand::ResetTextScale => {
                    tracing::debug!("Reset text scale");
                    self.reset_size();
                }
            }
        }
    }
}

fn init() {
    setup_logging();

    std::panic::set_hook(Box::new(custom_panic_hook));
    let program = Program::current();
    let text_request_va = program.rva_to_va(TEXT_RENDER_REQUEST_RVA).unwrap();
    unsafe {
        DrawTextRenderRequest
            .initialize(
                transmute::<u64, unsafe extern "C" fn(usize, *mut HavokPosition, *const u16)>(
                    text_request_va,
                ),
                |_ez_draw: usize, pos: *mut HavokPosition, text: *const u16| {
                    let text_str = u16_ptr_to_string(text);
                    let x = (*pos).0;
                    let y = (*pos).1;

                    TEXT_RENDER_QUEUE.force_push(DrawCommand::Text(text_str, x, y));
                },
            )
            .unwrap()
            .enable()
            .unwrap();
    }
    let set_font_size_va = program.rva_to_va(SET_FONT_SIZE_RVA).unwrap();
    unsafe {
        SetFontSize
            .initialize(
                transmute::<u64, unsafe extern "C" fn(usize, f32)>(set_font_size_va),
                |ez_draw: usize, font_size: f32| {
                    SetFontSize.call(ez_draw, font_size);
                    TEXT_RENDER_QUEUE.force_push(DrawCommand::SetFontSize(font_size));
                },
            )
            .unwrap()
            .enable()
            .unwrap();
    }
    let set_text_scale_va = program.rva_to_va(SET_TEXT_SCALE_RVA).unwrap();
    unsafe {
        SetTextScale
            .initialize(
                transmute::<u64, unsafe extern "C" fn(usize, f32, f32, f32)>(set_text_scale_va),
                |ez_draw: usize, width_scale: f32, height_scale: f32, font_size: f32| {
                    SetTextScale.call(ez_draw, width_scale, height_scale, font_size);
                    TEXT_RENDER_QUEUE.force_push(DrawCommand::SetTextScale(
                        width_scale,
                        height_scale,
                        font_size,
                    ));
                },
            )
            .unwrap()
            .enable()
            .unwrap();
    }
    let reset_text_scale_va = program.rva_to_va(RESET_TEXT_SCALE_RVA).unwrap();
    unsafe {
        ResetTextScale
            .initialize(
                transmute::<u64, unsafe extern "C" fn(usize)>(reset_text_scale_va),
                |ez_draw: usize| {
                    ResetTextScale.call(ez_draw);
                    TEXT_RENDER_QUEUE.force_push(DrawCommand::ResetTextScale);
                },
            )
            .unwrap()
            .enable()
            .unwrap();
    }
    let draw_text_with_size_va = program.rva_to_va(DRAW_TEXT_WITH_SIZE_RVA).unwrap();
    unsafe {
        DrawTextWithSize
            .initialize(
                transmute::<
                    u64,
                    unsafe extern "C" fn(usize, *mut HavokPosition, *mut f32, *const u16),
                >(draw_text_with_size_va),
                |_ez_draw: usize,
                 pos: *mut HavokPosition,
                 font_size_ptr: *mut f32,
                 text: *const u16| {
                    let text_str = u16_ptr_to_string(text);
                    let x = (*pos).0;
                    let y = (*pos).1;

                    let font_size = *font_size_ptr;
                    TEXT_RENDER_QUEUE.force_push(DrawCommand::SetFontSize(font_size));

                    TEXT_RENDER_QUEUE.force_push(DrawCommand::Text(text_str, x, y));
                },
            )
            .unwrap()
            .enable()
            .unwrap();
    }

    std::thread::spawn(|| {
        let program = Program::current();
        wait_for_system_init(&program, Duration::MAX).expect("System initialization timed out");

        if let Err(e) = Hudhook::builder()
            .with::<ImguiDx12Hooks>(DebugTextRender::new())
            .build()
            .apply()
        {
            tracing::error!("Failed to apply ImGui hooks: {:?}", e);
        }
    });
}

/// DLL entry point function.
///
/// # Safety
/// This function is safe to call when it's invoked by the Windows loader with valid parameters
/// during DLL loading, unloading, and thread attach/detach events.
#[unsafe(no_mangle)]
#[allow(non_snake_case)]
pub unsafe extern "C" fn DllMain(hinst: HINSTANCE, reason: u32, _reserved: usize) -> bool {
    if reason == DLL_PROCESS_ATTACH {
        unsafe { DisableThreadLibraryCalls(hinst).ok() };

        init();
    };
    true
}
