#include "bindings.h"
#include <stdio.h>
#include <node.h>
#include <v8.h>

#ifdef __unix__
#include <string.h>
#endif

static
v8::Local<v8::Value> V8LocalValueFromJsValue(napi_value v) {
  v8::Local<v8::Value> local;
  memcpy(&local, &v, sizeof(v));
  return local;
}

void extras_open_callback_scope(napi_async_context napi_async_context,
                                           napi_value napi_resource_object,
                                           extras_callback_scope* result) {
  v8::Isolate* isolate = v8::Isolate::GetCurrent();
  v8::Local<v8::Context> context = isolate->GetCurrentContext();
  v8::Local<v8::Object> resource_object = V8LocalValueFromJsValue(napi_resource_object)->ToObject(context).ToLocalChecked();
  node::async_context* node_async_context = reinterpret_cast<node::async_context*>(napi_async_context);
  *result = reinterpret_cast<extras_callback_scope>(new node::CallbackScope(isolate, resource_object, *node_async_context));
}

void extras_close_callback_scope(extras_callback_scope callback_scope) {
  delete reinterpret_cast<node::CallbackScope*>(callback_scope);
}
