#pragma once

#include <QJSEngine>
#include <QQmlEngine>

// For QML_SINGLETON types whose single instance is created in main() (so it
// can receive constructor arguments) and handed to the QML engine.
// The type must NOT be default-constructible: Qt prefers the default
// constructor over create() and would silently make a second instance.
#define SONKORE_QML_SINGLETON_INSTANCE(Type)                                                         \
public:                                                                                          \
    static Type* create(QQmlEngine*, QJSEngine*)                                                 \
    {                                                                                            \
        Q_ASSERT_X(s_instance, #Type, "setInstance() must be called before QML is loaded");     \
        QJSEngine::setObjectOwnership(s_instance, QJSEngine::CppOwnership);                      \
        return s_instance;                                                                       \
    }                                                                                            \
    static void setInstance(Type* instance) { s_instance = instance; }                           \
    static Type* instance() { return s_instance; }                                               \
                                                                                                 \
private:                                                                                         \
    static inline Type* s_instance = nullptr;
