/* Regression test for the engine-owned mirror window (renderers/oap_window.c), Windows only:
 * a window that was maximized when a stream ended must reappear (still maximized) for the next stream;
 * a stream starting shows the window without taking keyboard focus; the close button hides it and
 * calls the close handler (which ends the client session in the engine).
 * Runs itself again on a private desktop so no window flashes on the user's screen. */
#include <stdio.h>
#include <string.h>
#include <windows.h>
#include "oap_window.h"

static int wait_for(HWND h, BOOL want_visible) {
    for (int i = 0; i < 100; i++) {   /* up to 2 s: the window thread handles posted messages */
        if ((IsWindowVisible(h) != 0) == want_visible) return 1;
        Sleep(20);
    }
    return 0;
}

/* true when the mirror window is the active (keyboard-focus) window of its thread or the foreground window */
static int took_focus(HWND h) {
    GUITHREADINFO gti = { .cbSize = sizeof(gti) };
    DWORD tid = GetWindowThreadProcessId(h, NULL);
    if (GetGUIThreadInfo(tid, &gti) && (gti.hwndActive == h || gti.hwndFocus == h)) return 1;
    return GetForegroundWindow() == h;
}

static volatile LONG closes = 0;
static void on_close(void) { InterlockedIncrement(&closes); }

static int child(void) {
    oap_window_init("open-airplay reshow test");
    HWND h = (HWND) oap_window_handle();
    if (!h) { printf("FAIL: no window\n"); return 1; }

    oap_window_video_size(886, 1920);                 /* stream 1 starts */
    if (!wait_for(h, TRUE)) { printf("FAIL: window not shown for first stream\n"); return 1; }
    Sleep(100);
    if (took_focus(h)) { printf("FAIL: window took keyboard focus when the stream started\n"); return 1; }
    ShowWindow(h, SW_MAXIMIZE);                        /* user maximizes it */
    if (!IsZoomed(h)) { printf("FAIL: could not maximize\n"); return 1; }

    oap_window_hide();                                 /* stream 1 ends */
    if (!wait_for(h, FALSE)) { printf("FAIL: window not hidden at stream end\n"); return 1; }

    oap_window_video_size(886, 1920);                 /* stream 2 starts */
    if (!wait_for(h, TRUE)) { printf("FAIL: maximized window did not reappear for the next stream\n"); return 1; }
    if (!IsZoomed(h)) { printf("FAIL: window lost its maximized state\n"); return 1; }
    Sleep(100);
    if (took_focus(h)) { printf("FAIL: maximized window took keyboard focus when it reappeared\n"); return 1; }

    /* Full screen (F11 / double-click / this call) covers the monitor and comes back to the same place. */
    ShowWindow(h, SW_RESTORE);
    RECT before, full, after;
    GetWindowRect(h, &before);
    oap_window_toggle_fullscreen();
    Sleep(300);
    GetWindowRect(h, &full);
    MONITORINFO mi = { .cbSize = sizeof(mi) };
    GetMonitorInfoW(MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST), &mi);
    if (!EqualRect(&full, &mi.rcMonitor)) { printf("FAIL: full screen doesn't cover the monitor\n"); return 1; }
    oap_window_hide();                                 /* stream ends while full screen */
    if (!wait_for(h, FALSE)) { printf("FAIL: full-screen window not hidden at stream end\n"); return 1; }
    oap_window_video_size(1920, 886);                 /* next stream */
    if (!wait_for(h, TRUE)) { printf("FAIL: full-screen window did not reappear for the next stream\n"); return 1; }
    GetWindowRect(h, &full);
    if (!EqualRect(&full, &mi.rcMonitor)) { printf("FAIL: window left full screen on the next stream\n"); return 1; }
    oap_window_toggle_fullscreen();
    Sleep(300);
    GetWindowRect(h, &after);
    if (!EqualRect(&before, &after)) { printf("FAIL: leaving full screen didn't restore the window\n"); return 1; }

    /* The close button hides the window and runs the close handler. */
    oap_window_set_close_handler(on_close);
    PostMessageW(h, WM_CLOSE, 0, 0);
    if (!wait_for(h, FALSE)) { printf("FAIL: close button did not hide the window\n"); return 1; }
    Sleep(100);
    if (closes != 1) { printf("FAIL: close handler ran %ld times, expected 1\n", closes); return 1; }
    if (!IsWindow(h)) { printf("FAIL: close button destroyed the window\n"); return 1; }
    oap_window_video_size(886, 1920);                 /* the next stream still gets the window */
    if (!wait_for(h, TRUE)) { printf("FAIL: window did not reappear after being closed\n"); return 1; }

    printf("window reshow: all tests passed\n");
    return 0;
}

int main(int argc, char **argv) {
    if (argc > 1 && strcmp(argv[1], "--child") == 0) return child();

    HDESK desk = CreateDesktopW(L"oap-window-test", NULL, NULL, 0, GENERIC_ALL, NULL);
    if (!desk) { printf("FAIL: CreateDesktop error %lu\n", GetLastError()); return 1; }
    wchar_t self[MAX_PATH], cmd[MAX_PATH + 16];
    GetModuleFileNameW(NULL, self, MAX_PATH);
    swprintf(cmd, MAX_PATH + 16, L"\"%ls\" --child", self);
    STARTUPINFOW si = { .cb = sizeof(si), .lpDesktop = L"oap-window-test" };
    PROCESS_INFORMATION pi;
    if (!CreateProcessW(NULL, cmd, NULL, NULL, FALSE, 0, NULL, NULL, &si, &pi)) {
        printf("FAIL: CreateProcess error %lu\n", GetLastError());
        return 1;
    }
    WaitForSingleObject(pi.hProcess, 30000);
    DWORD code = 1;
    GetExitCodeProcess(pi.hProcess, &code);
    CloseHandle(pi.hProcess); CloseHandle(pi.hThread); CloseDesktop(desk);
    return (int) code;
}
