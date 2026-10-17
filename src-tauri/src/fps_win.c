/* DWM ProcessAttribution：每个合成帧里，哪些进程产生了新画面。
   只计 DirtyGenerated，一次合成里同一进程只记一帧。 */

#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <evntrace.h>
#include <evntcons.h>
#include <stdint.h>
#include <stdlib.h>

#define DWM_EVENT_ATTRIBUTION 387
#define SESSION_NAME L"MengMeterFps"

static const GUID DWM_CORE = {
    0x9E9BBA3C, 0x2E38, 0x40CB, {0x99, 0xF4, 0x9E, 0x82, 0x81, 0x42, 0x51, 0x64}};

static TRACEHANDLE g_session;
static volatile LONG g_stopping;
static void (*g_ready)(void *);
static void (*g_dirty)(uint32_t, void *);
static void *g_user;

static const BYTE *skip_array(const BYTE *p, const BYTE *end) {
    ULONG count;
    if (p + sizeof(ULONG) > end) {
        return NULL;
    }
    count = *(const ULONG *)p;
    p += sizeof(ULONG);
    if (count > 128 || p + (size_t)count * sizeof(ULONG) > end) {
        return NULL;
    }
    return p + (size_t)count * sizeof(ULONG);
}

static void emit_generated(const BYTE *data, USHORT len) {
    const BYTE *p = data;
    const BYTE *end = data + len;
    ULONG count;
    ULONG i;
    uint32_t seen[64];
    ULONG nseen = 0;

    p = skip_array(p, end);
    if (!p || p + sizeof(ULONG) > end) {
        return;
    }
    count = *(const ULONG *)p;
    p += sizeof(ULONG);
    if (count > 64 || p + (size_t)count * sizeof(uint32_t) > end) {
        return;
    }
    for (i = 0; i < count; i++) {
        uint32_t pid = *(const uint32_t *)p;
        ULONG j;
        int dup = 0;
        p += sizeof(uint32_t);
        if (pid == 0 || !g_dirty) {
            continue;
        }
        for (j = 0; j < nseen; j++) {
            if (seen[j] == pid) {
                dup = 1;
                break;
            }
        }
        if (dup) {
            continue;
        }
        if (nseen < 64) {
            seen[nseen++] = pid;
        }
        g_dirty(pid, g_user);
    }
}

static void WINAPI on_event(PEVENT_RECORD rec) {
    if (!rec || rec->EventHeader.EventDescriptor.Id != DWM_EVENT_ATTRIBUTION || !rec->UserData) {
        return;
    }
    emit_generated((const BYTE *)rec->UserData, rec->UserDataLength);
}

static void fill_props(EVENT_TRACE_PROPERTIES *props, ULONG buf_size) {
    ZeroMemory(props, buf_size);
    props->Wnode.BufferSize = buf_size;
    props->Wnode.Flags = WNODE_FLAG_TRACED_GUID;
    props->Wnode.ClientContext = 1;
    props->LogFileMode = EVENT_TRACE_REAL_TIME_MODE;
    props->LoggerNameOffset = sizeof(EVENT_TRACE_PROPERTIES);
    wcscpy_s((wchar_t *)((BYTE *)props + props->LoggerNameOffset),
             (buf_size - props->LoggerNameOffset) / sizeof(wchar_t), SESSION_NAME);
}

void fps_stop(void) {
    ULONG buf_size = sizeof(EVENT_TRACE_PROPERTIES) + 512;
    EVENT_TRACE_PROPERTIES *props;
    TRACEHANDLE session;

    InterlockedExchange(&g_stopping, 1);
    session = g_session;
    if (!session) {
        return;
    }
    props = (EVENT_TRACE_PROPERTIES *)calloc(1, buf_size);
    if (!props) {
        return;
    }
    props->Wnode.BufferSize = buf_size;
    props->LoggerNameOffset = sizeof(EVENT_TRACE_PROPERTIES);
    ControlTraceW(session, SESSION_NAME, props, EVENT_TRACE_CONTROL_STOP);
    free(props);
}

int fps_run(void (*on_ready)(void *), void (*on_dirty)(uint32_t, void *), void *user) {
    ULONG name_bytes = (ULONG)((wcslen(SESSION_NAME) + 1) * sizeof(wchar_t));
    ULONG buf_size = sizeof(EVENT_TRACE_PROPERTIES) + name_bytes;
    EVENT_TRACE_PROPERTIES *props;
    TRACEHANDLE session = 0;
    TRACEHANDLE trace;
    EVENT_TRACE_LOGFILEW log;
    ULONG status;
    int rc = 1;

    g_ready = on_ready;
    g_dirty = on_dirty;
    g_user = user;

    props = (EVENT_TRACE_PROPERTIES *)calloc(1, buf_size);
    if (!props) {
        return 1;
    }
    fill_props(props, buf_size);
    ControlTraceW(0, SESSION_NAME, props, EVENT_TRACE_CONTROL_STOP);
    fill_props(props, buf_size);

    if (g_stopping) {
        free(props);
        return 1;
    }
    status = StartTraceW(&session, SESSION_NAME, props);
    if (status != ERROR_SUCCESS) {
        free(props);
        return 1;
    }
    g_session = session;
    if (g_stopping) {
        fps_stop();
        free(props);
        return 1;
    }

    status = EnableTraceEx2(session, &DWM_CORE, EVENT_CONTROL_CODE_ENABLE_PROVIDER, 0xFF, 0x20, 0, 0,
                             NULL);
    if (status != ERROR_SUCCESS) {
        fps_stop();
        free(props);
        return 1;
    }
    if (g_ready) {
        g_ready(user);
    }

    ZeroMemory(&log, sizeof(log));
    log.LoggerName = (LPWSTR)SESSION_NAME;
    log.ProcessTraceMode = PROCESS_TRACE_MODE_REAL_TIME | PROCESS_TRACE_MODE_EVENT_RECORD;
    log.EventRecordCallback = on_event;
    trace = OpenTraceW(&log);
    if (trace == INVALID_PROCESSTRACE_HANDLE) {
        fps_stop();
        free(props);
        return 1;
    }
    if (!g_stopping) {
        ProcessTrace(&trace, 1, NULL, NULL);
        rc = 0;
    }
    CloseTrace(trace);
    fps_stop();
    free(props);
    return rc;
}
