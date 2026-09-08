// 任务栏图标角标(Windows):任务完成/需要审核时在任务栏图标上叠一个
// 橙色圆点,提示用户回来处理。实现 = GDI 合成 32px 位图(应用图标 + 角
// 标)→ CreateIconIndirect → SendMessageW(WM_SETICON, ICON_BIG)。换回
// 原图 = 重新合成无角标位图(直接恢复 GetClassLongPtr 的原 HICON 也行,
// 但合成路径统一更稳:原句柄属于窗口类,Shell 管理生命周期,不动它)。
//
// 角标状态机:boidy 级 Arc<Mutex<BadgeState>>,set(kind) 记最新状态;
// clear_all 回无角标。多会话并存时只要有任一会话在"需要关注"状态就
// 保持角标——由调用方(main.rs 事件钩子)聚合:每次事件到达时扫壳内
// 会话表,有关注项就 set,没有就 clear。
//
// 图标来源:shell32 无关,直接用主窗口当前 ICON(任务栏大图标是窗口类
// 的 ICON_BIG/ICON_SMALL),用 GetIconInfo 拿到位图再叠角标,免读资源。
#![cfg(target_os = "windows")]

use std::sync::Mutex;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, HBITMAP, HGDIOBJ,
    DIB_RGB_COLORS, ReleaseDC, SelectObject,
};
use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, HICON, ICON_BIG, WM_SETICON};
use windows::Win32::UI::WindowsAndMessaging::SendMessageW;

/// 角标种类:完成(绿)/待处理(橙)。当前只对外用 attention 橙,
/// 两态枚举保留扩展位。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BadgeKind {
    /// 任务完成(绿点)
    Done,
    /// 需要审核/提问(橙点)
    Attention,
}

#[derive(Default)]
struct BadgeState {
    /// 当前角标(None = 无)。同一窗口重复设置同态不重复重绘。
    active: Option<BadgeKind>,
}

/// HICON 裸指针不是 Send,包 ISend 包一层(句柄值进程内全局有效,
/// 跨线程传递只是类型系统不认)。
static STATE: Mutex<Option<BadgeState>> = Mutex::new(None);
/// 我们上一次 CreateIconIndirect 出来的 HICON(值不是 HICON 类型,
/// 包 isize 免 Send 问题):只有它才被 DestroyIcon,换下的类图标句柄
/// (WM_SETICON 首次返回的)绝不能销毁——那是 Shell 持有的。
static LAST_ICON: Mutex<Option<isize>> = Mutex::new(None);


/// 主窗口是否"用户没盯着"(最小化 或 关到托盘隐藏)。角标/通知只在这种
/// 状态才弹,前台可见时弹是噪音。取不到按 false——宁可不弹也不误报。
pub fn is_minimized(hwnd_raw: isize) -> Option<bool> {
    let hwnd = HWND(hwnd_raw as *mut _);
    unsafe {
        let iconic = windows::Win32::UI::WindowsAndMessaging::IsIconic(hwnd).as_bool();
        // 关到托盘是 SW_HIDE(隐藏,非最小化):隐藏也视为"没盯着"
        let hidden = windows::Win32::UI::WindowsAndMessaging::IsWindowVisible(hwnd).as_bool() == false;
        Some(iconic || hidden)
    }
}

/// 基础图标(RGBA 像素)缓存:从内嵌 32x32 应用图标解码(任务栏大图标
/// 标准尺寸)。不用 GetClassLongPtrW 取类图标——Tauri 窗口的任务栏图标
/// 来自 exe 资源,类图标句柄通常为 0,取不到整个角标路径就静默失效
/// (2026-09-08 用户报障:角标永远不出现的根因)。
fn base_pixels() -> Option<[u8; 32 * 32 * 4]> {
    static BASE: std::sync::OnceLock<Option<[u8; 32 * 32 * 4]>> = std::sync::OnceLock::new();
    let v = BASE.get_or_init(|| {
        let Ok(rgb) = image::ImageReader::new(
            std::io::Cursor::new(include_bytes!("../icons/32x32.png")),
        )
        .decode()
        else {
            return None;
        };
        let rgba = rgb.into_rgba8();
        if rgba.width() != 32 || rgba.height() != 32 {
            return None;
        }
        rgba.into_raw().try_into().ok()
    });
    v.as_ref().copied()
}

/// 在任务栏图标上叠角标(Attention/Done)。重复同态幂等。
pub fn set_badge(hwnd: HWND, kind: BadgeKind) {
    unsafe {
        let mut guard = STATE.lock().unwrap();
        let state = guard.get_or_insert_with(BadgeState::default);
        if state.active == Some(kind) {
            return; // 幂等
        }
        state.active = Some(kind);
        drop(guard);
        apply_icon(hwnd, Some(kind));
    }
}

/// 清除角标(恢复原图标)。
pub fn clear_badge(hwnd: HWND) {
    unsafe {
        let mut guard = STATE.lock().unwrap();
        let changed = matches!(guard.as_ref(), Some(s) if s.active.is_some());
        if let Some(s) = guard.as_mut() {
            s.active = None;
        }
        drop(guard);
        if changed {
            apply_icon(hwnd, None);
        }
    }
}

/// GDI 合成:嵌入 32x32 应用图标(普通 RGBA)→ 叠角标圆点(纯色+白描边,
/// 抗锯齿)→ 32bpp DIB → CreateIconIndirect → WM_SETICON(ICON_BIG)。
/// 任务栏显示的就是 ICON_BIG;小图标不动(Alt-Tab 无角标可接受)。
unsafe fn apply_icon(hwnd: HWND, kind: Option<BadgeKind>) {
    const SZ: usize = 32;
    let px = match base_pixels() {
        Some(b) => b,
        None => {
            if kind.is_some() {
                eprintln!("[badge] 内嵌图标解码失败,角标不可用");
            }
            return;
        }
    };

    // 颜色(ARGB→RGB)
    let color: [u8; 3] = match kind {
        Some(BadgeKind::Attention) => [0xFF, 0xA5, 0x00], // 橙
        Some(BadgeKind::Done) => [0x4C, 0xC2, 0x80],      // 绿
        None => [0, 0, 0],
    };

    // Rust 侧叠角标圆点(抗锯齿):右下角,半径 8,白边 1px
    let mut px = px.to_vec();
    if let Some(_) = kind {
        let r = 8.0;
        let cx = (SZ - 10) as f32;
        let cy = (SZ - 10) as f32;
        for y in 0..SZ {
            for x in 0..SZ {
                let d = (((x as f32) - cx).powi(2) + ((y as f32) - cy).powi(2)).sqrt();
                // 白描边环(半径 r..r+1)
                let edge_a = if d <= r + 1.0 { r + 1.0 - d } else { 0.0 };
                // 本体圆(半径 r,中心实心)
                let fill_a = if d <= r { 1.0 } else { r - d + 1.0 };
                let fill_a = fill_a.clamp(0.0, 1.0);
                // 白边在 d 介于 (r, r+1] 时:外圈白,内圈本色;用 (r-d+1) 渐变
                let white_a = if d > r { edge_a } else { 0.0 };
                if fill_a <= 0.0 && white_a <= 0.0 {
                    continue;
                }
                let idx = (y * SZ + x) * 4;
                let base_a = px[idx + 3] as f32 / 255.0;
                // 目标前景色:本色 or 白(取 alpha 更大的叠加)
                let fg: [f32; 3] = {
                    let cr = color[0] as f32;
                    let cg = color[1] as f32;
                    let cb = color[2] as f32;
                    [
                        if white_a > 0.0 { cr * (1.0 - white_a) + 255.0 * white_a } else { cr },
                        if white_a > 0.0 { cg * (1.0 - white_a) + 255.0 * white_a } else { cg },
                        if white_a > 0.0 { cb * (1.0 - white_a) + 255.0 * white_a } else { cb },
                    ]
                };
                let fg_a = fill_a.max(white_a);
                let out_a = (base_a + fg_a * (1.0 - base_a)).clamp(0.0, 1.0);
                if out_a <= 0.0 {
                    continue;
                }
                for ch in 0..3 {
                    let src = px[idx + ch] as f32 * base_a;
                    let top = fg[ch] * fg_a;
                    px[idx + ch] = ((src + top) / out_a).min(255.0).round() as u8;
                }
                px[idx + 3] = (out_a * 255.0).round() as u8;
            }
        }
    }

    let screen_dc = GetDC(None);
    let mem_dc = CreateCompatibleDC(Some(screen_dc));
    let mut bmi = windows::Win32::Graphics::Gdi::BITMAPINFO {
        bmiHeader: windows::Win32::Graphics::Gdi::BITMAPINFOHEADER {
            biSize: std::mem::size_of::<windows::Win32::Graphics::Gdi::BITMAPINFOHEADER>() as u32,
            biWidth: SZ as i32,
            biHeight: -(SZ as i32), // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: windows::Win32::Graphics::Gdi::BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
    let dib = match CreateDIBSection(Some(screen_dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
        Ok(b) => b,
        Err(_) => {
            let _ = DeleteDC(mem_dc);
            let _ = ReleaseDC(None, screen_dc);
            return;
        }
    };
    std::ptr::copy_nonoverlapping(px.as_ptr(), bits.cast::<u8>(), px.len());
    let _ = SelectObject(mem_dc, HGDIOBJ(dib.0));
    let _ = DeleteDC(mem_dc);
    let _ = ReleaseDC(None, screen_dc);

    // DIB(带 alpha)→ HICON:32bpp 位图 + 1x1 空 mask(Windows Vista+ 按
    // alpha 渲染,mask 只给旧系统降级)
    // mask 给 null:32bpp 带 alpha,Vista+ 按 alpha 通道渲染,null mask 可接受
    // (应用最低 Win10,WebView2 也要求 Win10+)
    let hmask = HBITMAP(std::ptr::null_mut());
    let icon_info = windows::Win32::UI::WindowsAndMessaging::ICONINFO {
        fIcon: true.into(),
        xHotspot: 0,
        yHotspot: 0,
        hbmMask: hmask,
        hbmColor: dib,
    };
    let hicon = match windows::Win32::UI::WindowsAndMessaging::CreateIconIndirect(&icon_info) {
        Ok(h) => h,
        Err(e) => {
            let _ = DeleteObject(HGDIOBJ(dib.0));
            if !hmask.is_invalid() {
                let _ = DeleteObject(HGDIOBJ(hmask.0));
            }
            eprintln!("[badge] CreateIconIndirect 失败: {e}");
            return;
        }
    };
    // 换任务栏大图标:先销毁**我们自己创建的**上一个 HICON,
    // WM_SETICON 换下的类图标句柄不能碰(Shell 持有)
    {
        let mut last = LAST_ICON.lock().unwrap();
        if let Some(raw) = *last {
            let _ = DestroyIcon(HICON(raw as *mut _));
            *last = None;
        }
    }
    let _ = SendMessageW(hwnd, WM_SETICON, Some(WPARAM(ICON_BIG as usize)), Some(LPARAM(hicon.0 as isize)));
    *LAST_ICON.lock().unwrap() = Some(hicon.0 as isize);
    let _ = DeleteObject(HGDIOBJ(dib.0));
}
