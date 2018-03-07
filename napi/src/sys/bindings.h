#include <node_api.h>
#include <uv.h>
#include <string.h>

typedef struct extras_callback_scope__ *extras_callback_scope;

EXTERN_C_START

NAPI_EXTERN void extras_open_callback_scope(napi_async_context napi_async_context,
                                           napi_value napi_resource_object,
                                           extras_callback_scope* result);

NAPI_EXTERN void extras_close_callback_scope(extras_callback_scope callback_scope);

EXTERN_C_END
