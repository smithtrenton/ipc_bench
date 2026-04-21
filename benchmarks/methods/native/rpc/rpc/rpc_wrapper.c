#include <rpc.h>
#include <rpcdce.h>
#include <rpcndr.h>
#include <stdlib.h>
#include <string.h>

#include "ipc_bench_rpc.h"

void IpcBenchPingClient(handle_t binding, byte* request, byte* response, unsigned long length);

void* __RPC_USER midl_user_allocate(size_t size) {
    return malloc(size);
}

void __RPC_USER midl_user_free(void* pointer) {
    free(pointer);
}

void IpcBenchPing(handle_t binding, byte* request, byte* response, unsigned long length) {
    (void)binding;
    if (length > 0) {
        memcpy(response, request, length);
        response[0] = (byte)(response[0] + 1);
    }
}

__declspec(dllexport) int rpc_server_start(const char* endpoint) {
    RPC_STATUS status = RpcServerUseProtseqEpA(
        (RPC_CSTR)"ncalrpc",
        RPC_C_PROTSEQ_MAX_REQS_DEFAULT,
        (RPC_CSTR)endpoint,
        NULL
    );
    if (status != RPC_S_OK) {
        return (int)status;
    }

    status = RpcServerRegisterIf(ipc_bench_rpc_v1_0_s_ifspec, NULL, NULL);
    if (status != RPC_S_OK) {
        return (int)status;
    }

    status = RpcServerListen(1, RPC_C_LISTEN_MAX_CALLS_DEFAULT, 1);
    if (status != RPC_S_OK && status != RPC_S_ALREADY_LISTENING) {
        return (int)status;
    }

    return 0;
}

__declspec(dllexport) int rpc_server_stop(void) {
    RPC_STATUS status = RpcMgmtStopServerListening(NULL);
    if (status != RPC_S_OK && status != RPC_S_NOT_LISTENING) {
        return (int)status;
    }

    status = RpcServerUnregisterIf(ipc_bench_rpc_v1_0_s_ifspec, NULL, 0);
    if (status != RPC_S_OK && status != RPC_S_UNKNOWN_IF) {
        return (int)status;
    }

    return 0;
}

__declspec(dllexport) int rpc_client_connect(const char* endpoint, handle_t* binding) {
    RPC_STATUS status;
    RPC_CSTR string_binding = NULL;

    status = RpcStringBindingComposeA(
        NULL,
        (RPC_CSTR)"ncalrpc",
        NULL,
        (RPC_CSTR)endpoint,
        NULL,
        &string_binding
    );
    if (status != RPC_S_OK) {
        return (int)status;
    }

    status = RpcBindingFromStringBindingA(string_binding, binding);
    RpcStringFreeA(&string_binding);
    return (int)status;
}

__declspec(dllexport) void rpc_client_disconnect(handle_t* binding) {
    if (binding != NULL && *binding != NULL) {
        RpcBindingFree(binding);
    }
}

__declspec(dllexport) int rpc_client_roundtrip(
    handle_t binding,
    const unsigned char* request,
    unsigned char* response,
    unsigned long length
) {
    IpcBenchPingClient(binding, (byte*)request, (byte*)response, length);
    return 0;
}
