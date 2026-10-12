#include <jni.h>
#include <stdbool.h>
#include <stdint.h>

extern uint64_t p2pshare_controller_create(void);
extern void p2pshare_android_initialize(void *vm, void *context);
extern void p2pshare_controller_destroy(uint64_t handle);
extern void p2pshare_controller_start_send_wait(uint64_t handle, const char *file_paths_json);
extern void p2pshare_controller_start_send_to_ticket(
    uint64_t handle,
    const char *file_paths_json,
    const char *ticket
);
extern void p2pshare_controller_start_receive_target(
    uint64_t handle,
    const char *target,
    const char *output_dir
);
extern void p2pshare_controller_start_receive_listen(uint64_t handle, const char *output_dir);
extern void p2pshare_controller_start_receive_nearby(uint64_t handle, const char *output_dir);
extern void p2pshare_controller_set_device_name(uint64_t handle, const char *name);
extern bool p2pshare_controller_answer_offer(uint64_t handle, bool accept);
extern bool p2pshare_controller_has_relay(uint64_t handle);
extern void p2pshare_controller_rebind(uint64_t handle);
extern void p2pshare_controller_start_browse(uint64_t handle);
extern void p2pshare_controller_stop_browse(uint64_t handle);
extern const char *p2pshare_controller_poll_nearby_json(uint64_t handle);
extern const char *p2pshare_controller_poll_event_json(uint64_t handle);
extern void p2pshare_controller_cancel(uint64_t handle);
extern void p2pshare_free_cstring(const char *ptr);

static jboolean native_initialize(JNIEnv *env, jclass clazz, jobject context) {
    (void) clazz;
    // Called once under RustBindings' monitor before the first endpoint is built.
    static jobject application_context = NULL;
    if (application_context != NULL) return JNI_TRUE;
    if (context == NULL) return JNI_FALSE;
    JavaVM *vm = NULL;
    if ((*env)->GetJavaVM(env, &vm) != JNI_OK) return JNI_FALSE;
    jobject retained = (*env)->NewGlobalRef(env, context);
    if (retained == NULL) return JNI_FALSE;
    p2pshare_android_initialize(vm, retained);
    application_context = retained;
    return JNI_TRUE;
}

static jlong native_create_controller(JNIEnv *env, jclass clazz) {
    (void) env;
    (void) clazz;
    return (jlong) p2pshare_controller_create();
}

static void native_start_send_wait(
    JNIEnv *env,
    jclass clazz,
    jlong handle,
    jstring file_paths_json
) {
    (void) clazz;
    if (file_paths_json == NULL) return;
    const char *paths = (*env)->GetStringUTFChars(env, file_paths_json, NULL);
    if (paths == NULL) return;
    p2pshare_controller_start_send_wait((uint64_t) handle, paths);
    (*env)->ReleaseStringUTFChars(env, file_paths_json, paths);
}

static void native_start_send_to_ticket(
    JNIEnv *env,
    jclass clazz,
    jlong handle,
    jstring file_paths_json,
    jstring ticket
) {
    (void) clazz;
    if (file_paths_json == NULL || ticket == NULL) return;

    const char *paths = (*env)->GetStringUTFChars(env, file_paths_json, NULL);
    if (paths == NULL) return;

    const char *ticket_str = (*env)->GetStringUTFChars(env, ticket, NULL);
    if (ticket_str == NULL) {
        (*env)->ReleaseStringUTFChars(env, file_paths_json, paths);
        return;
    }

    p2pshare_controller_start_send_to_ticket((uint64_t) handle, paths, ticket_str);

    (*env)->ReleaseStringUTFChars(env, ticket, ticket_str);
    (*env)->ReleaseStringUTFChars(env, file_paths_json, paths);
}

static void native_start_receive_target(
    JNIEnv *env,
    jclass clazz,
    jlong handle,
    jstring target,
    jstring output_dir
) {
    (void) clazz;
    if (target == NULL || output_dir == NULL) return;

    const char *target_str = (*env)->GetStringUTFChars(env, target, NULL);
    if (target_str == NULL) return;

    const char *output = (*env)->GetStringUTFChars(env, output_dir, NULL);
    if (output == NULL) {
        (*env)->ReleaseStringUTFChars(env, target, target_str);
        return;
    }

    p2pshare_controller_start_receive_target((uint64_t) handle, target_str, output);

    (*env)->ReleaseStringUTFChars(env, output_dir, output);
    (*env)->ReleaseStringUTFChars(env, target, target_str);
}

static void native_start_receive_listen(
    JNIEnv *env,
    jclass clazz,
    jlong handle,
    jstring output_dir
) {
    (void) clazz;
    if (output_dir == NULL) return;

    const char *output = (*env)->GetStringUTFChars(env, output_dir, NULL);
    if (output == NULL) return;

    p2pshare_controller_start_receive_listen((uint64_t) handle, output);

    (*env)->ReleaseStringUTFChars(env, output_dir, output);
}

/* Calls `action` with a temporary UTF-8 copy of `value`. */
static void with_utf(
    JNIEnv *env,
    jlong handle,
    jstring value,
    void (*action)(uint64_t, const char *)
) {
    if (value == NULL) return;
    const char *text = (*env)->GetStringUTFChars(env, value, NULL);
    if (text == NULL) return;
    action((uint64_t) handle, text);
    (*env)->ReleaseStringUTFChars(env, value, text);
}

static void native_start_receive_nearby(JNIEnv *env, jclass clazz, jlong handle, jstring output_dir) {
    (void) clazz;
    with_utf(env, handle, output_dir, p2pshare_controller_start_receive_nearby);
}

static void native_set_device_name(JNIEnv *env, jclass clazz, jlong handle, jstring name) {
    (void) clazz;
    with_utf(env, handle, name, p2pshare_controller_set_device_name);
}

static jboolean native_answer_offer(JNIEnv *env, jclass clazz, jlong handle, jboolean accept) {
    (void) env;
    (void) clazz;
    return p2pshare_controller_answer_offer((uint64_t) handle, accept == JNI_TRUE) ? JNI_TRUE : JNI_FALSE;
}

static jboolean native_has_relay(JNIEnv *env, jclass clazz, jlong handle) {
    (void) env;
    (void) clazz;
    return p2pshare_controller_has_relay((uint64_t) handle) ? JNI_TRUE : JNI_FALSE;
}

static void native_rebind(JNIEnv *env, jclass clazz, jlong handle) {
    (void) env;
    (void) clazz;
    p2pshare_controller_rebind((uint64_t) handle);
}

static void native_start_browse(JNIEnv *env, jclass clazz, jlong handle) {
    (void) env;
    (void) clazz;
    p2pshare_controller_start_browse((uint64_t) handle);
}

static void native_stop_browse(JNIEnv *env, jclass clazz, jlong handle) {
    (void) env;
    (void) clazz;
    p2pshare_controller_stop_browse((uint64_t) handle);
}

static jstring take_json(JNIEnv *env, const char *json) {
    if (json == NULL) {
        return NULL;
    }
    jstring out = (*env)->NewStringUTF(env, json);
    p2pshare_free_cstring(json);
    return out;
}

static jstring native_poll_nearby(JNIEnv *env, jclass clazz, jlong handle) {
    (void) clazz;
    return take_json(env, p2pshare_controller_poll_nearby_json((uint64_t) handle));
}

static jstring native_poll_event(JNIEnv *env, jclass clazz, jlong handle) {
    (void) clazz;
    return take_json(env, p2pshare_controller_poll_event_json((uint64_t) handle));
}

static void native_cancel(JNIEnv *env, jclass clazz, jlong handle) {
    (void) env;
    (void) clazz;
    p2pshare_controller_cancel((uint64_t) handle);
}

static void native_destroy_controller(JNIEnv *env, jclass clazz, jlong handle) {
    (void) env;
    (void) clazz;
    p2pshare_controller_destroy((uint64_t) handle);
}

int p2pshare_jni_register(JavaVM *vm) {
    JNIEnv *env = NULL;
    if ((*vm)->GetEnv(vm, (void **) &env, JNI_VERSION_1_6) != JNI_OK) {
        return JNI_ERR;
    }

    jclass clazz = (*env)->FindClass(env, "com/akily/p2pshare/bridge/RustBindings");
    if (clazz == NULL) {
        return JNI_ERR;
    }

    static const JNINativeMethod methods[] = {
        {"nativeInitialize", "(Landroid/content/Context;)Z", (void *) native_initialize},
        {"nativeCreateController", "()J", (void *) native_create_controller},
        {"nativeDestroyController", "(J)V", (void *) native_destroy_controller},
        {"nativeStartSendWait", "(JLjava/lang/String;)V", (void *) native_start_send_wait},
        {"nativeStartSendToTicket", "(JLjava/lang/String;Ljava/lang/String;)V", (void *) native_start_send_to_ticket},
        {"nativeStartReceiveTarget", "(JLjava/lang/String;Ljava/lang/String;)V", (void *) native_start_receive_target},
        {"nativeStartReceiveListen", "(JLjava/lang/String;)V", (void *) native_start_receive_listen},
        {"nativeStartReceiveNearby", "(JLjava/lang/String;)V", (void *) native_start_receive_nearby},
        {"nativeSetDeviceName", "(JLjava/lang/String;)V", (void *) native_set_device_name},
        {"nativeAnswerOffer", "(JZ)Z", (void *) native_answer_offer},
        {"nativeHasRelay", "(J)Z", (void *) native_has_relay},
        {"nativeRebind", "(J)V", (void *) native_rebind},
        {"nativeStartBrowse", "(J)V", (void *) native_start_browse},
        {"nativeStopBrowse", "(J)V", (void *) native_stop_browse},
        {"nativePollNearby", "(J)Ljava/lang/String;", (void *) native_poll_nearby},
        {"nativePollEvent", "(J)Ljava/lang/String;", (void *) native_poll_event},
        {"nativeCancel", "(J)V", (void *) native_cancel},
    };

    if ((*env)->RegisterNatives(
        env,
        clazz,
        methods,
        (jint) (sizeof(methods) / sizeof(methods[0]))
    ) != JNI_OK) {
        return JNI_ERR;
    }

    return JNI_VERSION_1_6;
}
