import ctypes
import ctypes.wintypes
import time

# Win32 API Constants
GWL_EXSTYLE = -20
WS_EX_LAYERED = 0x00080000
LWA_ALPHA = 0x00000002

# Hotkey IDs
HOTKEY_ID_BASE = 100
# Key Modifiers
MOD_CONTROL = 0x0002
MOD_SHIFT = 0x0004

user32 = ctypes.windll.user32

def set_window_transparency(hwnd, alpha):
    """
    hwnd: window handle
    alpha: 0 (invisible) to 255 (opaque)
    """
    # 1. Get current extended style
    style = user32.GetWindowLongW(hwnd, GWL_EXSTYLE)
    
    # 2. Add WS_EX_LAYERED if not present
    if not (style & WS_EX_LAYERED):
        user32.SetWindowLongW(hwnd, GWL_EXSTYLE, style | WS_EX_LAYERED)
    
    # 3. Set transparency level
    # If alpha is 255, we can choose to remove WS_EX_LAYERED for better performance
    if alpha >= 255:
        user32.SetLayeredWindowAttributes(hwnd, 0, 255, LWA_ALPHA)
        # Optional: remove LAYERED style to restore native rendering performance
        # user32.SetWindowLongW(hwnd, GWL_EXSTYLE, style & ~WS_EX_LAYERED)
    else:
        user32.SetLayeredWindowAttributes(hwnd, 0, alpha, LWA_ALPHA)

def main():
    print("=== 极轻量窗口半透明修改器原型 ===")
    print("快捷键：")
    print("Ctrl + Shift + 1-9 : 设置透明度 (10% - 90%)")
    print("Ctrl + Shift + 0   : 恢复不透明 (100%)")
    print("Ctrl + Shift + Q   : 退出程序")
    print("================================")

    # Register Hotkeys: Ctrl+Shift+0 to Ctrl+Shift+9
    # 0x30 is '0', 0x31 is '1', ..., 0x39 is '9'
    for i in range(10):
        # Register keys 0-9 (virtual keys 0x30-0x39)
        if not user32.RegisterHotKey(None, HOTKEY_ID_BASE + i, MOD_CONTROL | MOD_SHIFT, 0x30 + i):
            print(f"无法注册热键 Ctrl+Shift+{i}")
    
    # Register 'Q' to quit (0x51 is 'Q')
    user32.RegisterHotKey(None, HOTKEY_ID_BASE + 99, MOD_CONTROL | MOD_SHIFT, 0x51)

    try:
        msg = ctypes.wintypes.MSG()
        while user32.GetMessageW(ctypes.byref(msg), None, 0, 0) != 0:
            if msg.message == 0x0312: # WM_HOTKEY
                hk_id = msg.wParam
                
                if hk_id == HOTKEY_ID_BASE + 99: # Quit
                    break
                
                # Get index 0-9
                idx = hk_id - HOTKEY_ID_BASE
                
                # Calculate alpha: 0 -> 255, 1 -> 25, 2 -> 51...
                if idx == 0:
                    alpha = 255
                else:
                    alpha = int(255 * (idx / 10.0))
                
                # Get current active window
                hwnd = user32.GetForegroundWindow()
                if hwnd:
                    window_text = ctypes.create_unicode_buffer(256)
                    user32.GetWindowTextW(hwnd, window_text, 256)
                    print(f"正在修改窗口: {window_text.value} -> 透明度: {idx*10 if idx!=0 else 100}%")
                    set_window_transparency(hwnd, alpha)

            user32.TranslateMessage(ctypes.byref(msg))
            user32.DispatchMessageW(ctypes.byref(msg))
            
    finally:
        # Unregister all hotkeys
        for i in range(10):
            user32.UnregisterHotKey(None, HOTKEY_ID_BASE + i)
        user32.UnregisterHotKey(None, HOTKEY_ID_BASE + 99)
        print("程序已退出。")

if __name__ == "__main__":
    main()
