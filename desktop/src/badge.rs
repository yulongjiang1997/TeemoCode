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

use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateCompatibleDC, CreateDIBSection, CreateSolidBrush, DeleteDC, DeleteObject,
    Ellipse, EndPaint, FillRect, GetDC, HBRUSH, HBITMAP, HGDIOBJ, PAINTSTRUCT, DIB_RGB_COLORS,
    ReleaseDC, SelectObject,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateIconIndirect, GetIconInfo, GetSystemMetrics, DestroyIcon, HICON, ICONINFO, ICON_BIG,
    ICON_SMALL, SM_CXICON, SM_CYICON, WM_SETICON,
};
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
#[derive(Clone, Copy)]
struct SendIcon(*mut std::ffi::c_void);
unsafe impl Send for SendIcon {}

static STATE: Mutex<Option<BadgeState>> = Mutex::new(None);
static BASE_ICON: Mutex<Option<SendIcon>> = Mutex::new(None);

/// 主窗口是否最小化(角标/通知只在用户没盯着时才弹,前台可见不骚扰)。
/// 取不到按"未最小化"(返回 false)——宁可不弹也不在前台弹噪音。
pub fn is_minimized(hwnd_raw: isize) -> Option<bool> {
    let hwnd = HWND(hwnd_raw as *mut _);
    Some(unsafe { windows::Win32::UI::WindowsAndMessaging::IsIconic(hwnd).as_bool() })
}

/// 记录窗口类原始大图标(首次 attach 时),clear 时恢复。
/// HICON 句柄值跨进程内复用安全(窗口类 ICON 由 Shell/内核持有)。
unsafe fn capture_base_icon(hwnd: HWND) {
    let mut guard = BASE_ICON.lock().unwrap();
    if guard.is_none() {
        let h = windows::Win32::UI::WindowsAndMessaging::GetClassLongPtrW(
            hwnd,
            windows::Win32::UI::WindowsAndMessaging::GCLP_HICON,
        );
        if h != 0 {
            *guard = Some(SendIcon(h as *mut _));
        }
    }
}

/// 在任务栏图标上叠角标(Attention/Done)。重复同态幂等。
pub fn set_badge(hwnd: HWND, kind: BadgeKind) {
    unsafe {
        capture_base_icon(hwnd);
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

/// GDI 合成:取当前大图标位图 → 复制 DIB → 右下角叠 12px 圆点(白描边)
/// → CreateIconIndirect → WM_SETICON(ICON_BIG)。小图标同步换(Alt-Tab /
/// 部分任务栏形态用小图)。
unsafe fn apply_icon(hwnd: HWND, kind: Option<BadgeKind>) {
    let cx = GetSystemMetrics(SM_CXICON).max(16);
    let cy = GetSystemMetrics(SM_CYICON).max(16);

    // 原图标位图(彩色掩码两份)
    let base = BASE_ICON.lock().unwrap();
    let Some(icon) = *base else { return };
    let base_hicon = HICON(icon.0);
    let mut info = ICONINFO {
        fIcon: true.into(),
        ..Default::default()
    };
    if GetIconInfo(base_hicon, &mut info).is_err() {
        return;
    }
    // info.hbmColor/hbmMask 需要手动释放(GetIconInfo 交出的句柄)
    let has_color = !info.hbmColor.is_invalid();
    drop(base);

    let screen_dc = GetDC(None);
    let mem_dc = CreateCompatibleDC(Some(screen_dc));

    // 目标 DIB:32bpp 预乘 alpha
    let mut bmi = windows::Win32::Graphics::Gdi::BITMAPINFO {
        bmiHeader: windows::Win32::Graphics::Gdi::BITMAPINFOHEADER {
            biSize: std::mem::size_of::<windows::Win32::Graphics::Gdi::BITMAPINFOHEADER>() as u32,
            biWidth: cx,
            biHeight: -cy, // top-down
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
            if has_color {
                let _ = DeleteObject(HGDIOBJ(info.hbmColor.0));
            }
            let _ = DeleteObject(HGDIOBJ(info.hbmMask.0));
            return;
        }
    };

    // 原图标画到 DIB(有彩色位图 DrawIconEx;无彩色退化为纯色方块,可接受)
    let old = SelectObject(mem_dc, HGDIOBJ(dib.0));
    if has_color {
        let _ = windows::Win32::UI::WindowsAndMessaging::DrawIconEx(
            mem_dc,
            0,
            0,
            base_hicon,
            cx,
            cy,
            0,
            None,
            windows::Win32::UI::WindowsAndMessaging::DI_NORMAL,
        );
        // DrawIconEx 在 32bpp DIB 上可能清 alpha(同 native_pet 的经验):
        // 拉回不透明。透明像素(alpha=0 且 RGB=0)保留透明。
        let slice = std::slice::from_raw_parts_mut(bits.cast::<u8>(), (cx * cy * 4) as usize);
        for px in slice.chunks_exact_mut(4) {
            if px[3] == 0 && (px[0] != 0 || px[1] != 0 || px[2] != 0) {
                px[3] = 255;
            } else if px[3] != 0 && px[3] != 255 {
                // 预乘修正:GDI 画的图标是未预乘,直接当 opaque 用
                px[0] = px[0].min(255);
                px[1] = px[1].min(255);
                px[2] = px[2].min(255);
                px[3] = 255;
            }
        }
    } else {
        // 兜底:纯色底(base-200 蓝灰),仍可辨
        let brush = CreateSolidBrush(COLORREF(0x00E0E0E0)); // BGR 浅灰
        let rc = RECT { left: 0, top: 0, right: cx, bottom: cy };
        let _ = windows::Win32::Graphics::Gdi::FillRect(mem_dc, &rc, brush);
        let _ = DeleteObject(HGDIOBJ(brush.0));
    }

    // 角标圆点:右下角,半径 = 尺寸 22%,白描边 1px
    if let Some(kind) = kind {
        let r = (cx as f64 * 0.22).round() as i32;
        let cxp = cx - r - (cx as f64 * 0.08).round() as i32;
        let cyp = cy - r - (cy as f64 * 0.08).round() as i32;
        let color = match kind {
            BadgeKind::Attention => COLORREF(0x0000A5FF), // BGR 橙 #FFA500
            BadgeKind::Done => COLORREF(0x0080C24C),     // BGR 绿 #4CC280
        };
        // 白描边:先画大一号白圆,再画本色圆(GDI Ellipse 用当前选中的 brush)
        let edge = CreateSolidBrush(COLORREF(0x00FFFFFF));
        let prev_brush = SelectObject(mem_dc, HGDIOBJ(edge.0));
        let _ = Ellipse(mem_dc, cxp - r - 1, cyp - r - 1, cxp + r + 1, cyp + r + 1);
        let fill = CreateSolidBrush(color);
        let _ = SelectObject(mem_dc, HGDIOBJ(fill.0));
        let _ = Ellipse(mem_dc, cxp - r, cyp - r, cxp + r, cyp + r);
        let _ = SelectObject(mem_dc, prev_brush);
        let _ = DeleteObject(HGDIOBJ(edge.0));
        let _ = DeleteObject(HGDIOBJ(fill.0));
    }

    SelectObject(mem_dc, old);
    let _ = DeleteDC(mem_dc);
    let _ = ReleaseDC(None, screen_dc);

    // DIB → HICON:mask 透明处理(fIcon=TRUE 且 hbmMask 空=按 alpha)。
    // CreateIconIndirect 对 32bpp DIB:hbmColor 位图带 alpha 时 mask 传 1x1
    // 全零即可。
    let icon_info = ICONINFO {
        fIcon: true.into(),
        xHotspot: 0,
        yHotspot: 0,
        hbmMask: if has_color {
            HBITMAP(std::ptr::null_mut())
        } else {
            info.hbmMask
        },
        hbmColor: dib,
    };
    let hicon = match CreateIconIndirect(&icon_info) {
        Ok(h) => h,
        Err(_) => {
            let _ = DeleteObject(HGDIOBJ(dib.0));
            if has_color {
                let _ = DeleteObject(HGDIOBJ(info.hbmColor.0));
            }
            let _ = DeleteObject(HGDIOBJ(info.hbmMask.0));
            return;
        }
    };

    // 换窗口大图标(任务栏显示 ICON_BIG)+ 小图标(Alt-Tab)
    // ICON_SMALL 也要换,否则部分形态的任务栏用小图标,角标看不出来。
    // 换下来的旧 HICON 要销毁(我们自己 CreateIconIndirect 出来的句柄)。
    let old_big = SendMessageW(hwnd, WM_SETICON, Some(WPARAM(ICON_BIG as usize)), Some(LPARAM(hicon.0 as isize)));
    let old_small = SendMessageW(hwnd, WM_SETICON, Some(WPARAM(ICON_SMALL as usize)), Some(LPARAM(hicon.0 as isize)));
    // 上一轮合成的 HICON 已被换下,销毁;首轮换下的是类图标句柄(Shell 持有),
    // 不销毁——用 ORIGINAL_CLASS_ICON 哨兵跳过。简化:首轮 old 是类句柄,
    // DestroyIcon 对类句柄是安全的 no-op(文档:仅销毁 CreateIcon 系创建的)。
    if old_big.0 != 0 {
        let _ = DestroyIcon(HICON(old_big.0 as *mut _));
    }
    if old_small.0 != 0 && old_small.0 != old_big.0 {
        let _ = DestroyIcon(HICON(old_small.0 as *mut _));
    }

    // 清理素材
    let _ = DeleteObject(HGDIOBJ(dib.0));
    if has_color {
        let _ = DeleteObject(HGDIOBJ(info.hbmColor.0));
    }
    let _ = DeleteObject(HGDIOBJ(info.hbmMask.0));
}
