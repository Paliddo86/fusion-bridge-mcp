// Included by sdk.cpp so the context and retained-document ABI remain private.
// This file performs typed property/method calls only. Rust owns operation
// planning, JSON, revisions, result classification and rollback policy.
#include <Core/Application/Products.h>
#include <Core/Application/UnitsManager.h>
#include <Core/Application/ValueInput.h>
#include <Fusion/Fusion/Design.h>
#include <Fusion/Components/Component.h>
#include <Fusion/Components/Components.h>
#include <Fusion/Fusion/Parameter.h>
#include <Fusion/Fusion/ParameterList.h>
#include <Fusion/Fusion/UserParameter.h>
#include <Fusion/Fusion/UserParameters.h>
#include <Fusion/Fusion/Timeline.h>
#include <Fusion/Fusion/TimelineObject.h>
#include <Fusion/BRep/BRepBodies.h>
#include <Fusion/Sketch/Sketches.h>
#include <cmath>
#include <string>

static bool af_input(const char* value, size_t len) {
    if ((!value && len != 0) || len > 65536) return false;
    for (size_t i = 0; i < len; ++i) if (value[i] == '\0') return false;
    return true;
}
static std::string af_string(const char* value, size_t len) {
    return len ? std::string(value, len) : std::string();
}
static int af_design(AfContext* context, AfDocumentRef* document, Ptr<adsk::fusion::Design>& out) {
    int result = check_document(context, document); if (result) return result;
    if (!valid(context)) return 3;
    if (!document->document->isValid() || context->app->getLastError() != 0) return 3;
    auto products = document->document->products();
    if (context->app->getLastError() != 0 || !products) return 7;
    auto product = products->itemByProductType("DesignProductType");
    if (context->app->getLastError() != 0 || !product) return 7;
    out = product;
    if (!out || !out->isValid() || context->app->getLastError() != 0) return 3;
    auto owner = out->parentDocument();
    if (context->app->getLastError() != 0 || owner != document->document) return 3;
    return 0;
}
static int af_active_document(AfContext* context, AfDocumentRef* document) {
    auto active = context->app->activeDocument();
    if (context->app->getLastError() != 0) return 7;
    return active == document->document ? 0 : 6;
}
extern "C" int af_design_state(AfContext* context, AfDocumentRef* document, AfDesignState* out, AfBytes sink, void* user) noexcept {
    if (!out || !sink) return 1;
    *out = {};
    try {
        Ptr<adsk::fusion::Design> design;
        int status = af_design(context, document, design); if (status) return status;
        auto active = context->app->activeDocument();
        if (context->app->getLastError() != 0) return 7;
        out->active = active == document->document ? 1 : 0;
        auto type = design->designType();
        if (context->app->getLastError() != 0) return 7;
        if (type != adsk::fusion::DirectDesignType && type != adsk::fusion::ParametricDesignType) return 6;
        out->parametric = type == adsk::fusion::ParametricDesignType ? 1 : 0;
        auto root = design->rootComponent();
        if (context->app->getLastError() != 0 || !root) return 7;
        auto name = root->name();
        if (context->app->getLastError() != 0) return 7;
        auto revision = root->revisionId();
        if (context->app->getLastError() != 0 || revision.empty()) return 7;
        auto units = design->unitsManager();
        if (context->app->getLastError() != 0 || !units) return 7;
        auto default_units = units->defaultLengthUnits();
        if (context->app->getLastError() != 0 || default_units.empty()) return 7;
        auto components = design->allComponents();
        if (context->app->getLastError() != 0 || !components) return 7;
        out->component_count = components->count();
        if (context->app->getLastError() != 0) return 7;
        auto bodies = root->bRepBodies();
        if (context->app->getLastError() != 0 || !bodies) return 7;
        out->body_count = bodies->count();
        if (context->app->getLastError() != 0) return 7;
        auto sketches = root->sketches();
        if (context->app->getLastError() != 0 || !sketches) return 7;
        out->sketch_count = sketches->count();
        if (context->app->getLastError() != 0) return 7;
        if (out->parametric) {
            auto timeline = design->timeline();
            if (context->app->getLastError() != 0 || !timeline) return 7;
            out->timeline_count = timeline->count();
            if (context->app->getLastError() != 0) return 7;
            auto position = timeline->markerPosition();
            if (context->app->getLastError() != 0 || position < 0) return 7;
            out->timeline_position = static_cast<uint64_t>(position);
        }
        if (!sink(user, 0, name.data(), name.size())) return 5;
        if (!sink(user, 1, revision.data(), revision.size())) return 5;
        if (!sink(user, 2, default_units.data(), default_units.size())) return 5;
        return 0;
    } catch (...) { return 4; }
}
extern "C" int af_parameter_count(AfContext* context, AfDocumentRef* document, uint64_t* out) noexcept {
    if (!out) return 1;
    *out = 0;
    try {
        Ptr<adsk::fusion::Design> design;
        int status = af_design(context, document, design); if (status) return status;
        auto parameters = design->allParameters();
        if (context->app->getLastError() != 0 || !parameters) return 7;
        *out = parameters->count();
        return context->app->getLastError() == 0 ? 0 : 7;
    } catch (...) { return 4; }
}
extern "C" int af_parameter_read(AfContext* context, AfDocumentRef* document, uint64_t index, AfParameter* out, AfBytes sink, void* user) noexcept {
    if (!out || !sink) return 1;
    *out = {};
    try {
        Ptr<adsk::fusion::Design> design;
        int status = af_design(context, document, design); if (status) return status;
        auto parameters = design->allParameters();
        if (context->app->getLastError() != 0 || !parameters) return 7;
        auto count = parameters->count();
        if (context->app->getLastError() != 0) return 7;
        if (index >= count || index > SIZE_MAX) return 6;
        auto parameter = parameters->item(static_cast<size_t>(index));
        if (context->app->getLastError() != 0 || !parameter) return 7;
        if (!parameter->isValid() || context->app->getLastError() != 0) return 3;
        auto name = parameter->name();
        if (context->app->getLastError() != 0) return 7;
        auto expression = parameter->expression();
        if (context->app->getLastError() != 0) return 7;
        auto unit = parameter->unit();
        if (context->app->getLastError() != 0) return 7;
        auto comment = parameter->comment();
        if (context->app->getLastError() != 0) return 7;
        out->is_deletable = parameter->isDeletable() ? 1 : 0;
        if (context->app->getLastError() != 0) return 7;
        const char* object_type = parameter->objectType();
        if (!object_type || context->app->getLastError() != 0) return 7;
        out->is_user = std::string(object_type) == adsk::fusion::UserParameter::classType() ? 1 : 0;
        auto value_type = parameter->valueType();
        if (context->app->getLastError() != 0) return 7;
        std::string text;
        if (value_type == adsk::fusion::NumericParameterValueType) {
            out->value_type = 0;
            out->value = parameter->value();
            if (context->app->getLastError() != 0 || !std::isfinite(out->value)) return 7;
        } else if (value_type == adsk::fusion::TextParameterValueType) {
            out->value_type = 1;
            text = parameter->textValue();
            if (context->app->getLastError() != 0) return 7;
        } else { return 6; }
        if (!sink(user, 0, name.data(), name.size())) return 5;
        if (!sink(user, 1, expression.data(), expression.size())) return 5;
        if (!sink(user, 2, unit.data(), unit.size())) return 5;
        if (!sink(user, 3, comment.data(), comment.size())) return 5;
        if (!sink(user, 4, text.data(), text.size())) return 5;
        return 0;
    } catch (...) { return 4; }
}
extern "C" int af_timeline_health(AfContext* context, AfDocumentRef* document, uint64_t index, uint32_t* out, AfBytes sink, void* user) noexcept {
    if (!out || !sink) return 1;
    try {
        Ptr<adsk::fusion::Design> design;
        int status = af_design(context, document, design); if (status) return status;
        auto timeline = design->timeline();
        if (context->app->getLastError() != 0 || !timeline) return 7;
        auto count = timeline->count();
        if (context->app->getLastError() != 0) return 7;
        if (index >= count || index > SIZE_MAX) return 6;
        auto object = timeline->item(static_cast<size_t>(index));
        if (context->app->getLastError() != 0 || !object) return 7;
        auto health = object->healthState();
        if (context->app->getLastError() != 0) return 7;
        *out = static_cast<uint32_t>(health);
        std::string message;
        if (health == adsk::fusion::WarningFeatureHealthState || health == adsk::fusion::ErrorFeatureHealthState) {
            message = object->errorOrWarningMessage();
            if (context->app->getLastError() != 0) return 7;
        }
        return sink(user, 0, message.data(), message.size()) ? 0 : 5;
    } catch (...) { return 4; }
}
extern "C" int af_parameter_evaluate(AfContext* context, AfDocumentRef* document, const char* expression, size_t expression_len, const char* unit, size_t unit_len, double* out) noexcept {
    if (!out || !af_input(expression, expression_len) || !af_input(unit, unit_len)) return 1;
    try {
        Ptr<adsk::fusion::Design> design;
        int status = af_design(context, document, design); if (status) return status;
        auto units = design->unitsManager();
        if (context->app->getLastError() != 0 || !units) return 7;
        auto expr = af_string(expression, expression_len), unit_string = af_string(unit, unit_len);
        auto valid_expression = units->isValidExpression(expr, unit_string);
        if (context->app->getLastError() != 0) return 7;
        if (!valid_expression) return 20;
        *out = units->evaluateExpression(expr, unit_string);
        if (context->app->getLastError() != 0 || !std::isfinite(*out)) return 7;
        return 0;
    } catch (...) { return 4; }
}
extern "C" int af_parameter_create(AfContext* context, AfDocumentRef* document, const char* name, size_t name_len, const char* expression, size_t expression_len, const char* unit, size_t unit_len, const char* comment, size_t comment_len) noexcept {
    if (!af_input(name, name_len) || !af_input(expression, expression_len) || !af_input(unit, unit_len) || !af_input(comment, comment_len)) return 1;
    try {
        Ptr<adsk::fusion::Design> design;
        int status = af_design(context, document, design); if (status) return status;
        status = af_active_document(context, document); if (status) return status;
        auto users = design->userParameters();
        if (context->app->getLastError() != 0 || !users) return 7;
        auto value = adsk::core::ValueInput::createByString(af_string(expression, expression_len));
        if (context->app->getLastError() != 0 || !value) return 7;
        auto parameter = users->add(af_string(name, name_len), value, af_string(unit, unit_len), af_string(comment, comment_len));
        if (context->app->getLastError() != 0 || !parameter) return 7;
        return parameter->isValid() && context->app->getLastError() == 0 ? 0 : 7;
    } catch (...) { return 4; }
}
extern "C" int af_parameter_set(AfContext* context, AfDocumentRef* document, const char* name, size_t name_len, const char* expression, size_t expression_len) noexcept {
    if (!af_input(name, name_len) || !af_input(expression, expression_len)) return 1;
    try {
        Ptr<adsk::fusion::Design> design;
        int status = af_design(context, document, design); if (status) return status;
        status = af_active_document(context, document); if (status) return status;
        auto parameters = design->allParameters();
        if (context->app->getLastError() != 0 || !parameters) return 7;
        auto parameter = parameters->itemByName(af_string(name, name_len));
        if (context->app->getLastError() != 0 || !parameter) return 7;
        auto changed = parameter->expression(af_string(expression, expression_len));
        return changed && context->app->getLastError() == 0 ? 0 : 7;
    } catch (...) { return 4; }
}
extern "C" int af_design_compute(AfContext* context, AfDocumentRef* document) noexcept {
    try {
        Ptr<adsk::fusion::Design> design;
        int status = af_design(context, document, design); if (status) return status;
        status = af_active_document(context, document); if (status) return status;
        auto computed = design->computeAll();
        return computed && context->app->getLastError() == 0 ? 0 : 7;
    } catch (...) { return 4; }
}
