#include "sdk.h"
#include <Windows.h>
#include <Core/Application/Application.h>
#include <Core/Application/Documents.h>
#include <Core/Application/Document.h>
#include <limits>
#include <new>

using adsk::core::Application;
using adsk::core::Ptr;
struct AfContext { DWORD thread; Ptr<Application> app; };
// Copying an SDK Ptr retains the same Document interface. Never adopt a borrowed
// raw Document* (Ptr(raw) defaults to attach=true). No identity policy lives here.
struct AfDocumentRef { AfContext* owner; Ptr<adsk::core::Document> document; };
static int check(AfContext* context) noexcept {
    if (!context) return 1;
    if (context->thread != GetCurrentThreadId()) return 2;
    return 0;
}
// SDK errors are returned values, not exceptions. Check immediately so a later
// successful call cannot hide a failed property read. An old uncleared error
// conservatively refuses this probe; no undocumented reset is attempted.
static bool valid(AfContext* context) {
    return context->app && context->app->isValid() && context->app->getLastError() == 0;
}
extern "C" uint32_t af_thread_id() noexcept { return GetCurrentThreadId(); }
extern "C" int af_open(AfContext** out) noexcept {
    if (!out) return 1;
    *out = nullptr;
    try {
        auto app = Application::get();
        if (!app || !app->isValid() || app->getLastError() != 0) return 3;
        *out = new AfContext{GetCurrentThreadId(), app};
        return 0;
    } catch (...) { return 4; }
}
extern "C" int af_close(AfContext* context) noexcept {
    if (!context) return 1;
    if (context->thread != GetCurrentThreadId()) return 2;
    try { delete context; return 0; } catch (...) { return 4; }
}
extern "C" int af_version(AfContext* context, AfBytes sink, void* user) noexcept {
    if (!sink) return 1;
    int result = check(context); if (result) return result;
    try {
        if (!valid(context)) return 3;
        auto value = context->app->version();
        if (context->app->getLastError() != 0 || value.empty()) return 7;
        return sink(user, 0, value.data(), value.size()) ? 0 : 5;
    } catch (...) { return 4; }
}
extern "C" int af_document_count(AfContext* context, uint64_t* out) noexcept {
    if (!out) return 1;
    int result = check(context); if (result) return result;
    try {
        if (!valid(context)) return 3;
        auto docs = context->app->documents();
        if (context->app->getLastError() != 0 || !docs) return 7;
        if (!docs->isValid() || context->app->getLastError() != 0) return 3;
        *out = docs->count();
        if (context->app->getLastError() != 0) return 7;
        return 0;
    } catch (...) { return 4; }
}
extern "C" int af_document_retain(AfContext* context, uint64_t index, AfDocumentRef** out) noexcept {
    if (!out) return 1;
    *out = nullptr;
    int result = check(context); if (result) return result;
    try {
        if (!valid(context)) return 3;
        auto docs = context->app->documents();
        if (context->app->getLastError() != 0 || !docs) return 7;
        if (!docs->isValid() || context->app->getLastError() != 0) return 3;
        auto count = docs->count();
        if (context->app->getLastError() != 0) return 7;
        if (index >= count || index > INT_MAX) return 6;
        auto doc = docs->item(static_cast<size_t>(index));
        if (context->app->getLastError() != 0 || !doc) return 7;
        if (!doc->isValid() || context->app->getLastError() != 0) return 3;
        *out = new AfDocumentRef{context, doc};
        return 0;
    } catch (...) { return 4; }
}
static int check_document(AfContext* context, AfDocumentRef* document) noexcept {
    int result = check(context); if (result) return result;
    if (!document || document->owner != context || !document->document) return 1;
    return 0;
}
extern "C" int af_document_same(AfContext* context, AfDocumentRef* left, AfDocumentRef* right, uint32_t* out) noexcept {
    if (!out) return 1;
    int result = check_document(context, left); if (result) return result;
    result = check_document(context, right); if (result) return result;
    // Memory.h compares retained same-static-type Ptr objects by their underlying
    // interface pointer. Equality performs no SDK getter or ownership transfer.
    *out = left->document == right->document ? 1 : 0;
    return 0;
}
extern "C" int af_document_valid(AfContext* context, AfDocumentRef* document, uint32_t* out) noexcept {
    if (!out) return 1;
    int result = check_document(context, document); if (result) return result;
    try {
        if (!valid(context)) return 3;
        const bool value = document->document->isValid();
        if (context->app->getLastError() != 0) return 7;
        *out = value ? 1 : 0;
        return 0;
    } catch (...) { return 4; }
}
extern "C" int af_document_release(AfContext* context, AfDocumentRef* document) noexcept {
    int result = check(context); if (result) return result;
    if (!document || document->owner != context) return 1;
    // A failed create can return its preallocated holder with an empty Ptr.
    // The holder is still owned and must be released exactly once.
    // Invalid/closed Fusion documents must still release their retained lease.
    // Do not call isValid or require an error-free Application during cleanup.
    try { delete document; return 0; } catch (...) { return 4; }
}
extern "C" int af_document_ref_read(AfContext* context, AfDocumentRef* document, AfDocument* out, AfBytes sink, void* user) noexcept {
    if (!out || !sink) return 1;
    int result = check_document(context, document); if (result) return result;
    try {
        if (!valid(context)) return 3;
        const auto& doc = document->document;
        if (!doc->isValid() || context->app->getLastError() != 0) return 3;
        auto active = context->app->activeDocument();
        if (context->app->getLastError() != 0) return 7;
        out->active = doc == active ? 1 : 0;
        out->saved = doc->isSaved() ? 1 : 0;
        if (context->app->getLastError() != 0) return 7;
        out->modified = doc->isModified() ? 1 : 0;
        if (context->app->getLastError() != 0) return 7;
        auto name = doc->name();
        if (context->app->getLastError() != 0) return 7;
        auto creation = doc->creationId();
        if (context->app->getLastError() != 0 || creation.empty()) return 7;
        if (!sink(user, 0, name.data(), name.size())) return 5;
        if (!sink(user, 1, creation.data(), creation.size())) return 5;
        return 0;
    } catch (...) { return 4; }
}

#include "mechanical.inc"
#include "document_lifecycle.inc"
#include "bom.inc"
#include "sketch.inc"
#include "sketch_geometry.inc"
#include "extrusion.inc"

#include "materials.inc"

#include "purchased.inc"
#include "physical.inc"
#include "line_state.inc"

#include "relations.inc"
#include "dimensions.inc"

#include "assembly.inc"
#include "placement.inc"
