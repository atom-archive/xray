#include <node_api.h>

napi_value init_module(napi_env env, napi_value exports);

NAPI_MODULE(PROTON, init_module)
