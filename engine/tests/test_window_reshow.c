/* Regression test for the engine-owned mirror window (renderers/oap_window.c), Windows only:
 * a window that was maximized when a stream ended must reappear (still maximized) for the next stream.
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

static int child(void) {
    oap_window_init("open-airplay reshow test");
    HWND h = (HWND) oap_window_handle();
    if (!h) { printf("FAIL: no window\n"); return 1; }

    oap_window_video_size(886, 1920);                 /* stream 1 starts */
    if (!wait_for(h, TRUE)) { printf("FAIL: window not shown for first stream\n"); return 1; }
    ShowWindow(h, SW_MAXIMIZE);                        /* user maximizes it */
    if (!IsZoomed(h)) { printf("FAIL: could not maximize\n"); return 1; }

    oap_window_hide();                                 /* stream 1 ends */
    if (!wait_for(h, FALSE)) { printf("FAIL: window not hidden at stream end\n"); return 1; }

    oap_window_video_size(886, 1920);                 /* stream 2 starts */
    if (!wait_for(h, TRUE)) { printf("FAIL: maximized window did not reappear for the next stream\n"); return 1; }
    if (!IsZoomed(h)) { printf("FAIL: window lost its maximized state\n"); return 1; }

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
