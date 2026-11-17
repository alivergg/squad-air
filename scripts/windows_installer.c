#define UNICODE
#define _UNICODE
#define COBJMACROS
#define WIN32_LEAN_AND_MEAN

#include <windows.h>
#include <shlobj.h>
#include <objbase.h>
#include <shellapi.h>
#include <tlhelp32.h>
#include <wchar.h>

#define APP_NAME L"SquadAir"
#define APP_VERSION L"0.1.0"
#define RESOURCE_PAYLOAD_ID 101
#define IDC_INSTALL_BTN 1001
#define IDC_CANCEL_BTN 1002
#define IDC_STATUS_LBL 1003

static HINSTANCE g_hinst = NULL;
static HWND g_hwnd_main = NULL;
static HWND g_hwnd_install = NULL;
static HWND g_hwnd_status = NULL;
static HFONT g_font_title = NULL;
static HFONT g_font_body = NULL;

static wchar_t g_install_dir[MAX_PATH] = {0};
static wchar_t g_exe_path[MAX_PATH] = {0};
static BOOL g_is_upgrade = FALSE;

static void init_install_paths(void) {
    wchar_t local_appdata[MAX_PATH] = {0};
    if (!SHGetSpecialFolderPathW(NULL, local_appdata, CSIDL_LOCAL_APPDATA, TRUE)) {
        GetEnvironmentVariableW(L"LOCALAPPDATA", local_appdata, MAX_PATH);
    }
    swprintf(g_install_dir, MAX_PATH, L"%ls\\Programs\\%ls", local_appdata, APP_NAME);
    swprintf(g_exe_path, MAX_PATH, L"%ls\\SquadAir.exe", g_install_dir);

    DWORD attrs = GetFileAttributesW(g_exe_path);
    g_is_upgrade = (attrs != INVALID_FILE_ATTRIBUTES && !(attrs & FILE_ATTRIBUTE_DIRECTORY));
}

static void ensure_install_dir(void) {
    wchar_t local_appdata[MAX_PATH] = {0};
    if (!SHGetSpecialFolderPathW(NULL, local_appdata, CSIDL_LOCAL_APPDATA, TRUE)) {
        GetEnvironmentVariableW(L"LOCALAPPDATA", local_appdata, MAX_PATH);
    }
    wchar_t programs_dir[MAX_PATH] = {0};
    swprintf(programs_dir, MAX_PATH, L"%ls\\Programs", local_appdata);
    CreateDirectoryW(programs_dir, NULL);
    CreateDirectoryW(g_install_dir, NULL);
}

static void kill_running_instances(void) {
    DWORD self_pid = GetCurrentProcessId();
    HANDLE snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
    if (snap == INVALID_HANDLE_VALUE) {
        return;
    }

    PROCESSENTRY32W pe;
    pe.dwSize = sizeof(pe);
    if (Process32FirstW(snap, &pe)) {
        do {
            if (pe.th32ProcessID == self_pid) {
                continue;
            }
            if (_wcsicmp(pe.szExeFile, L"SquadAir.exe") == 0 ||
                _wcsicmp(pe.szExeFile, L"Squad-Air.exe") == 0 ||
                _wcsicmp(pe.szExeFile, L"squad-client.exe") == 0) {
                HANDLE hproc = OpenProcess(PROCESS_TERMINATE | SYNCHRONIZE, FALSE, pe.th32ProcessID);
                if (hproc) {
                    TerminateProcess(hproc, 0);
                    WaitForSingleObject(hproc, 3000);
                    CloseHandle(hproc);
                }
            }
        } while (Process32NextW(snap, &pe));
    }
    CloseHandle(snap);
    Sleep(150);
}

static BOOL extract_payload_atomically(void) {
    HRSRC res = FindResourceW(g_hinst, MAKEINTRESOURCEW(RESOURCE_PAYLOAD_ID), MAKEINTRESOURCEW(10));
    if (!res) {
        return FALSE;
    }
    DWORD size = SizeofResource(g_hinst, res);
    HGLOBAL loaded = LoadResource(g_hinst, res);
    if (!loaded || size == 0) {
        return FALSE;
    }
    const void *data = LockResource(loaded);
    if (!data) {
        return FALSE;
    }

    wchar_t tmp_path[MAX_PATH] = {0};
    swprintf(tmp_path, MAX_PATH, L"%ls\\SquadAir.exe.tmp", g_install_dir);

    HANDLE hf = CreateFileW(
        tmp_path,
        GENERIC_WRITE,
        0,
        NULL,
        CREATE_ALWAYS,
        FILE_ATTRIBUTE_NORMAL,
        NULL
    );
    if (hf == INVALID_HANDLE_VALUE) {
        return FALSE;
    }

    const BYTE *ptr = (const BYTE *)data;
    DWORD remaining = size;
    BOOL write_ok = TRUE;
    while (remaining > 0) {
        DWORD chunk = 0;
        if (!WriteFile(hf, ptr, remaining, &chunk, NULL) || chunk == 0) {
            write_ok = FALSE;
            break;
        }
        ptr += chunk;
        remaining -= chunk;
    }
    FlushFileBuffers(hf);
    CloseHandle(hf);

    if (!write_ok) {
        DeleteFileW(tmp_path);
        return FALSE;
    }

    if (!MoveFileExW(tmp_path, g_exe_path, MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH)) {
        DeleteFileW(tmp_path);
        return FALSE;
    }
    return TRUE;
}

static void create_shortcut(const wchar_t *lnk_path, const wchar_t *target_exe, const wchar_t *work_dir) {
    IShellLinkW *psl = NULL;
    HRESULT hr = CoCreateInstance(
        &CLSID_ShellLink,
        NULL,
        CLSCTX_INPROC_SERVER,
        &IID_IShellLinkW,
        (void **)&psl
    );
    if (FAILED(hr) || !psl) {
        return;
    }

    IShellLinkW_SetPath(psl, target_exe);
    IShellLinkW_SetWorkingDirectory(psl, work_dir);
    IShellLinkW_SetDescription(psl, L"SquadAir Encrypted P2P Voice & Screen Share");
    IShellLinkW_SetIconLocation(psl, target_exe, 0);

    IPersistFile *ppf = NULL;
    if (SUCCEEDED(IShellLinkW_QueryInterface(psl, &IID_IPersistFile, (void **)&ppf)) && ppf) {
        IPersistFile_Save(ppf, lnk_path, TRUE);
        IPersistFile_Release(ppf);
    }
    IShellLinkW_Release(psl);
}

static void create_start_menu_shortcut(void) {
    wchar_t programs[MAX_PATH] = {0};
    if (SHGetSpecialFolderPathW(NULL, programs, CSIDL_PROGRAMS, TRUE)) {
        wchar_t lnk[MAX_PATH] = {0};
        swprintf(lnk, MAX_PATH, L"%ls\\SquadAir.lnk", programs);
        create_shortcut(lnk, g_exe_path, g_install_dir);
    }
}

static void create_desktop_shortcut(void) {
    wchar_t desktop[MAX_PATH] = {0};
    if (SHGetSpecialFolderPathW(NULL, desktop, CSIDL_DESKTOPDIRECTORY, TRUE)) {
        wchar_t lnk[MAX_PATH] = {0};
        swprintf(lnk, MAX_PATH, L"%ls\\SquadAir.lnk", desktop);
        create_shortcut(lnk, g_exe_path, g_install_dir);
    }
}

static void write_uninstall_registry(void) {
    HKEY hkey = NULL;
    if (RegCreateKeyExW(
            HKEY_CURRENT_USER,
            L"Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\SquadAir",
            0,
            NULL,
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            NULL,
            &hkey,
            NULL) != ERROR_SUCCESS) {
        return;
    }

    RegSetValueExW(hkey, L"DisplayName", 0, REG_SZ, (const BYTE *)APP_NAME,
                   (DWORD)((wcslen(APP_NAME) + 1) * sizeof(wchar_t)));
    RegSetValueExW(hkey, L"DisplayVersion", 0, REG_SZ, (const BYTE *)APP_VERSION,
                   (DWORD)((wcslen(APP_VERSION) + 1) * sizeof(wchar_t)));
    RegSetValueExW(hkey, L"Publisher", 0, REG_SZ, (const BYTE *)L"SquadAir Open Source",
                   (DWORD)((wcslen(L"SquadAir Open Source") + 1) * sizeof(wchar_t)));
    RegSetValueExW(hkey, L"InstallLocation", 0, REG_SZ, (const BYTE *)g_install_dir,
                   (DWORD)((wcslen(g_install_dir) + 1) * sizeof(wchar_t)));
    RegSetValueExW(hkey, L"DisplayIcon", 0, REG_SZ, (const BYTE *)g_exe_path,
                   (DWORD)((wcslen(g_exe_path) + 1) * sizeof(wchar_t)));
    DWORD one = 1;
    RegSetValueExW(hkey, L"NoModify", 0, REG_DWORD, (const BYTE *)&one, sizeof(one));
    RegSetValueExW(hkey, L"NoRepair", 0, REG_DWORD, (const BYTE *)&one, sizeof(one));
    RegCloseKey(hkey);
}

static void run_installation(HWND hwnd) {
    EnableWindow(g_hwnd_install, FALSE);
    if (g_hwnd_status) {
        SetWindowTextW(g_hwnd_status, L"Closing previous version and extracting files...");
    }

    kill_running_instances();
    ensure_install_dir();

    if (!extract_payload_atomically()) {
        MessageBoxW(
            hwnd,
            L"Failed to write SquadAir.exe. Please close the running application manually and try again.",
            L"SquadAir Setup Error",
            MB_OK | MB_ICONERROR
        );
        EnableWindow(g_hwnd_install, TRUE);
        return;
    }

    CoInitializeEx(NULL, COINIT_APARTMENTTHREADED);
    create_start_menu_shortcut();
    write_uninstall_registry();

    const wchar_t *done_msg = g_is_upgrade
        ? L"SquadAir was successfully updated!\n\nCreate a Desktop shortcut?"
        : L"SquadAir was successfully installed!\n\nCreate a Desktop shortcut?";

    int add_desktop = MessageBoxW(
        hwnd,
        done_msg,
        L"SquadAir Setup",
        MB_YESNO | MB_ICONQUESTION
    );
    if (add_desktop == IDYES) {
        create_desktop_shortcut();
    }
    CoUninitialize();

    int launch_now = MessageBoxW(
        hwnd,
        L"Launch SquadAir now?",
        L"SquadAir Setup",
        MB_YESNO | MB_ICONINFORMATION
    );
    if (launch_now == IDYES) {
        ShellExecuteW(NULL, L"open", g_exe_path, NULL, g_install_dir, SW_SHOWNORMAL);
    }

    PostQuitMessage(0);
}

static LRESULT CALLBACK wnd_proc(HWND hwnd, UINT msg, WPARAM wp, LPARAM lp) {
    switch (msg) {
    case WM_CREATE: {
        g_font_title = CreateFontW(
            -21, 0, 0, 0, FW_SEMIBOLD, FALSE, FALSE, FALSE,
            DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY, DEFAULT_PITCH | FF_SWISS, L"Segoe UI"
        );
        g_font_body = CreateFontW(
            -14, 0, 0, 0, FW_NORMAL, FALSE, FALSE, FALSE,
            DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY, DEFAULT_PITCH | FF_SWISS, L"Segoe UI"
        );

        const wchar_t *header_text = g_is_upgrade
            ? L"Update SquadAir v" APP_VERSION
            : L"Install SquadAir v" APP_VERSION;

        const wchar_t *desc_text = g_is_upgrade
            ? L"An existing SquadAir installation was detected.\nYour application will be updated in place while keeping your settings and saved channels."
            : L"Minimal one-click setup.\nInstall location: %LOCALAPPDATA%\\Programs\\SquadAir";

        const wchar_t *btn_text = g_is_upgrade ? L"Update" : L"Install";

        HWND h_title = CreateWindowW(
            L"STATIC", header_text,
            WS_CHILD | WS_VISIBLE,
            24, 20, 390, 28,
            hwnd, NULL, g_hinst, NULL
        );
        SendMessageW(h_title, WM_SETFONT, (WPARAM)g_font_title, TRUE);

        g_hwnd_status = CreateWindowW(
            L"STATIC", desc_text,
            WS_CHILD | WS_VISIBLE,
            24, 58, 392, 62,
            hwnd, (HMENU)IDC_STATUS_LBL, g_hinst, NULL
        );
        SendMessageW(g_hwnd_status, WM_SETFONT, (WPARAM)g_font_body, TRUE);

        g_hwnd_install = CreateWindowW(
            L"BUTTON", btn_text,
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_DEFPUSHBUTTON,
            196, 146, 118, 34,
            hwnd, (HMENU)IDC_INSTALL_BTN, g_hinst, NULL
        );
        SendMessageW(g_hwnd_install, WM_SETFONT, (WPARAM)g_font_body, TRUE);

        HWND h_cancel = CreateWindowW(
            L"BUTTON", L"Cancel",
            WS_CHILD | WS_VISIBLE | WS_TABSTOP,
            322, 146, 94, 34,
            hwnd, (HMENU)IDC_CANCEL_BTN, g_hinst, NULL
        );
        SendMessageW(h_cancel, WM_SETFONT, (WPARAM)g_font_body, TRUE);
        return 0;
    }
    case WM_COMMAND:
        if (LOWORD(wp) == IDC_INSTALL_BTN) {
            run_installation(hwnd);
        } else if (LOWORD(wp) == IDC_CANCEL_BTN) {
            PostQuitMessage(0);
        }
        return 0;
    case WM_DESTROY:
        if (g_font_title) {
            DeleteObject(g_font_title);
        }
        if (g_font_body) {
            DeleteObject(g_font_body);
        }
        PostQuitMessage(0);
        return 0;
    }
    return DefWindowProcW(hwnd, msg, wp, lp);
}

int WINAPI wWinMain(HINSTANCE hInstance, HINSTANCE hPrevInstance, PWSTR lpCmdLine, int nCmdShow) {
    (void)hPrevInstance;
    (void)lpCmdLine;
    g_hinst = hInstance;

    init_install_paths();

    const wchar_t *cls_name = L"SquadAirSetupWindow";
    WNDCLASSW wc = {0};
    wc.lpfnWndProc = wnd_proc;
    wc.hInstance = hInstance;
    wc.hbrBackground = (HBRUSH)(COLOR_WINDOW + 1);
    wc.hCursor = LoadCursorW(NULL, IDC_ARROW);
    wc.lpszClassName = cls_name;
    RegisterClassW(&wc);

    int width = 446;
    int height = 226;
    int x = (GetSystemMetrics(SM_CXSCREEN) - width) / 2;
    int y = (GetSystemMetrics(SM_CYSCREEN) - height) / 2;

    g_hwnd_main = CreateWindowExW(
        WS_EX_DLGMODALFRAME,
        cls_name,
        L"SquadAir Setup",
        WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
        x, y, width, height,
        NULL, NULL, hInstance, NULL
    );
    if (!g_hwnd_main) {
        return 1;
    }

    ShowWindow(g_hwnd_main, nCmdShow);
    UpdateWindow(g_hwnd_main);

    MSG msg;
    while (GetMessageW(&msg, NULL, 0, 0) > 0) {
        if (!IsDialogMessageW(g_hwnd_main, &msg)) {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    return 0;
}
