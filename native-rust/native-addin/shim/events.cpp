#include "sdk.h"
#include <Windows.h>
#include <Core/Application/Application.h>
#include <Core/Application/CustomEvents.h>
#include <atomic>
#include <string>

using adsk::core::Application;
using adsk::core::CustomEvent;
using adsk::core::CustomEventArgs;
using adsk::core::CustomEventHandler;
using adsk::core::Ptr;

class WakeHandler final : public CustomEventHandler {
public:
    WakeHandler(AfWake callback, void* user) : callback_(callback), user_(user), thread_(GetCurrentThreadId()) {}
    void notify(const Ptr<CustomEventArgs>&) override {
        // The event is just a wake signal. It can never carry executable code,
        // an object handle, a request or an authority claim from another add-in.
        if (enabled.load() && thread_ == GetCurrentThreadId()) callback_(user_);
    }
    std::atomic<bool> enabled{true};
private:
    AfWake callback_;
    void* user_;
    DWORD thread_;
};

struct AfEvent {
    AfEvent(AfWake callback, void* user, std::string id)
        : thread(GetCurrentThreadId()), id(std::move(id)), handler(callback, user) {}
    DWORD thread;
    std::string id;
    Ptr<Application> app;
    Ptr<CustomEvent> event;
    WakeHandler handler;
    bool registered = false;
    bool attached = false;
};

extern "C" int af_event_open(const char* id, size_t len, AfWake callback, void* user, AfEvent** out) noexcept {
    if (!out) return 1;
    *out = nullptr;
    if (!id || len == 0 || len > 128 || !callback || !user) return 1;
    try {
        // Returning a non-null handle on failure gives the owner a way to
        // reconcile partial registration. Never unregister a failed duplicate.
        *out = new AfEvent(callback, user, std::string(id, len));
        auto* state = *out;
        if (state->id.find('\0') != std::string::npos) return 1;
        state->app = Application::get();
        if (!state->app || !state->app->isValid() || state->app->getLastError() != 0) return 3;
        state->event = state->app->registerCustomEvent(state->id);
        if (!state->event) return 7;
        state->registered = true;
        if (state->app->getLastError() != 0) return 7;
        state->attached = state->event->add(&state->handler);
        if (!state->attached || state->app->getLastError() != 0) return 7;
        return 0;
    } catch (...) { return 4; }
}

extern "C" int af_event_fire(AfEvent* state) noexcept {
    if (!state) return 1;
    try {
        // fireCustomEvent is Autodesk's explicitly supported worker-thread
        // bridge. No other SDK getter, cast, reference update or release here.
        if (!state->handler.enabled.load()) return 8;
        return state->app->fireCustomEvent(state->id) ? 0 : 7;
    } catch (...) { return 4; }
}

extern "C" int af_event_close(AfEvent* state) noexcept {
    if (!state) return 1;
    if (state->thread != GetCurrentThreadId()) return 2;
    state->handler.enabled.store(false);
    try {
        // The owner has joined the workers. Keep both SDK references and the
        // handler alive on any refusal, until explicit reconciliation succeeds.
        if (state->attached) {
            if (!state->event->remove(&state->handler)) return 7;
            state->attached = false;
            if (state->app->getLastError() != 0) return 7;
        }
        if (state->registered) {
            if (!state->app->unregisterCustomEvent(state->id)) return 7;
            state->registered = false;
            if (state->app->getLastError() != 0) return 7;
        }
        delete state;
        return 0;
    } catch (...) { return 4; }
}
