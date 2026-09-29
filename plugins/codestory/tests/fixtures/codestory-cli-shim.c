// codestory-cli-shim.c — native Windows stand-in for the fixture CLI used by
// plugin-static.test.mjs.
//
// Production codestory-mcp.cjs deliberately rejects .cmd/.bat shims and spawns
// the resolved CLI with shell:false, so Windows fixtures need a real PE. This
// forwarder carries a trailer appended by writeNodeCli/writeFakeCli:
//
//   [shim.exe bytes][script UTF-8][node path UTF-8][trailer]
//   trailer = u32le scriptLen | u32le nodeLen | 8-byte magic "CSFCSHIM"
//
// At startup it extracts the script to %TEMP%, then execs
//   "<node>" "<temp script>" <original argument tail>
// so argv[1] is the script path exactly as the POSIX `node script "$@"`
// wrapper produces. Stdio handles are inherited so piped JSON-RPC works.
//
// Build (any Windows host with clang-cl or cl):
//   clang-cl /O2 /MT /W3 codestory-cli-shim.c
//   cl /O2 /MT /W3 codestory-cli-shim.c
// The resulting codestory-cli-shim.exe is committed next to this file; rebuild
// it and update the test-side provenance check if this source changes.

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <wchar.h>
#include <windows.h>

static const unsigned char SHIM_MAGIC[8] = { 'C', 'S', 'F', 'C', 'S', 'H', 'I', 'M' };
static const DWORD SHIM_TRAILER = 16; /* u32 scriptLen + u32 nodeLen + 8 magic */

static int fail(const wchar_t *stage)
{
    DWORD written = 0;
    wchar_t wide[256];
    char narrow[512];
    int length = swprintf(wide, 256,
        L"codestory-cli-shim failed at %ls\n", stage);
    int narrowLength = length > 0
        ? WideCharToMultiByte(CP_UTF8, 0, wide, length, narrow, 511, NULL, NULL)
        : 0;
    if (narrowLength > 0) {
        WriteFile(GetStdHandle(STD_ERROR_HANDLE), narrow,
            (DWORD)narrowLength, &written, NULL);
    }
    return 126;
}

int wmain(void)
{
    wchar_t ownPath[32768];
    DWORD pathLen = GetModuleFileNameW(NULL, ownPath, 32768);
    if (pathLen == 0 || pathLen >= 32768) return fail(L"GetModuleFileNameW");

    HANDLE file = CreateFileW(ownPath, GENERIC_READ, FILE_SHARE_READ,
        NULL, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, NULL);
    if (file == INVALID_HANDLE_VALUE) return fail(L"open self");

    LARGE_INTEGER size;
    if (!GetFileSizeEx(file, &size)) { CloseHandle(file); return fail(L"size"); }
    if (size.QuadPart < (LONGLONG)SHIM_TRAILER) { CloseHandle(file); return fail(L"trailer"); }

    unsigned char trailer[16];
    DWORD got = 0;
    LARGE_INTEGER off;
    off.QuadPart = size.QuadPart - SHIM_TRAILER;
    if (!SetFilePointerEx(file, off, NULL, FILE_BEGIN) ||
        !ReadFile(file, trailer, SHIM_TRAILER, &got, NULL) || got != SHIM_TRAILER) {
        CloseHandle(file);
        return fail(L"read trailer");
    }
    if (memcmp(trailer + 8, SHIM_MAGIC, 8) != 0) {
        CloseHandle(file);
        return fail(L"magic");
    }
    DWORD scriptLen = *(DWORD *)(trailer + 0);
    DWORD nodeLen = *(DWORD *)(trailer + 4);
    if ((LONGLONG)(scriptLen + nodeLen + SHIM_TRAILER) > size.QuadPart) {
        CloseHandle(file);
        return fail(L"bounds");
    }

    char *blob = (char *)HeapAlloc(GetProcessHeap(), 0, scriptLen + nodeLen);
    if (!blob) { CloseHandle(file); return fail(L"alloc"); }
    off.QuadPart = size.QuadPart - SHIM_TRAILER - nodeLen - scriptLen;
    if (!SetFilePointerEx(file, off, NULL, FILE_BEGIN) ||
        !ReadFile(file, blob, scriptLen + nodeLen, &got, NULL) ||
        got != scriptLen + nodeLen) {
        CloseHandle(file);
        return fail(L"read payload");
    }
    CloseHandle(file);
    const char *script = blob;
    const char *nodePathUtf8 = blob + scriptLen;

    int nodePathChars = MultiByteToWideChar(CP_UTF8, 0, nodePathUtf8, (int)nodeLen, NULL, 0);
    if (nodePathChars <= 0) return fail(L"node path");
    wchar_t *nodePath = (wchar_t *)HeapAlloc(GetProcessHeap(), 0,
        (nodePathChars + 1) * sizeof(wchar_t));
    if (!nodePath) return fail(L"node alloc");
    MultiByteToWideChar(CP_UTF8, 0, nodePathUtf8, (int)nodeLen, nodePath, nodePathChars);
    nodePath[nodePathChars] = 0;

    wchar_t tempDir[MAX_PATH];
    DWORD tempLen = GetTempPathW(MAX_PATH, tempDir);
    if (tempLen == 0 || tempLen >= MAX_PATH) return fail(L"temp dir");
    wchar_t scriptPath[MAX_PATH * 2];
    swprintf(scriptPath, MAX_PATH * 2, L"%ls\\codestory-fake-cli-%lu.cjs",
        tempDir, GetCurrentProcessId());
    for (wchar_t *p = scriptPath; *p; p += 1) {
        if (*p == L'/' ) *p = L'\\';
    }
    HANDLE scriptFile = CreateFileW(scriptPath, GENERIC_WRITE, 0, NULL,
        CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, NULL);
    if (scriptFile == INVALID_HANDLE_VALUE) return fail(L"temp script");
    if (!WriteFile(scriptFile, script, scriptLen, &got, NULL) || got != scriptLen) {
        CloseHandle(scriptFile);
        return fail(L"write script");
    }
    CloseHandle(scriptFile);

    /* Rebuild the command line: "node" "script" <original tail after argv0>. */
    LPCWSTR cmdline = GetCommandLineW();
    LPCWSTR tail = cmdline;
    if (*tail == L'"') {
        tail += 1;
        while (*tail && *tail != L'"') tail += 1;
        if (*tail) tail += 1;
    } else {
        while (*tail && *tail != L' ' && *tail != L'\t') tail += 1;
    }
    while (*tail == L' ' || *tail == L'\t') tail += 1;

    size_t commandLen = nodePathChars + wcslen(scriptPath) + wcslen(tail) + 8;
    wchar_t *command = (wchar_t *)HeapAlloc(GetProcessHeap(), 0,
        (commandLen + 1) * sizeof(wchar_t));
    if (!command) return fail(L"command alloc");
    swprintf(command, commandLen + 1, L"\"%ls\" \"%ls\" %ls",
        nodePath, scriptPath, tail);

    STARTUPINFOW si;
    memset(&si, 0, sizeof(si));
    si.cb = sizeof(si);
    si.dwFlags = STARTF_USESTDHANDLES;
    si.hStdInput = GetStdHandle(STD_INPUT_HANDLE);
    si.hStdOutput = GetStdHandle(STD_OUTPUT_HANDLE);
    si.hStdError = GetStdHandle(STD_ERROR_HANDLE);
    PROCESS_INFORMATION pi;
    memset(&pi, 0, sizeof(pi));
    if (!CreateProcessW(NULL, command, NULL, NULL, TRUE, 0, NULL, NULL, &si, &pi)) {
        DeleteFileW(scriptPath);
        return fail(L"CreateProcessW");
    }
    WaitForSingleObject(pi.hProcess, INFINITE);
    DWORD exitCode = 1;
    GetExitCodeProcess(pi.hProcess, &exitCode);
    CloseHandle(pi.hThread);
    CloseHandle(pi.hProcess);
    DeleteFileW(scriptPath);
    return (int)exitCode;
}
