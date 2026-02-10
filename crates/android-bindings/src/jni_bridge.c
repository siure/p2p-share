#include <jni.h>
#include <stdint.h>

extern uint64_t p2pshare_controller_create(void);
extern void p2pshare_controller_start_send_wait(uint64_t handle, const char *file_path);
extern void p2pshare_controller_start_send_to_ticket(
    uint64_t handle,
    const char *file_path,
    const char *ticket
);
extern void p2pshare_controller_start_receive_target(
    uint64_t handle,
    const char *target,
    const char *output_dir
);
extern void p2pshare_controller_start_receive_listen(uint64_t handle, const char *output_dir);
extern const char *p2pshare_controller_poll_event_json(uint64_t handle);
extern void p2pshare_controller_cancel(uint64_t handle);
extern void p2pshare_free_cstring(const char *ptr);

static jlong native_create_controller(JNIEnv *env, jclass clazz) {
    (void) env;
    (void) clazz;
    return (jlong) p2pshare_controller_create();
}

static void native_start_send_wait(
    JNIEnv *env,
    jclass clazz,
    jlong handle,
    jstring file_path
) {
    (void) clazz;
    if (file_path == NULL) return;
    const char *path = (*env)->GetStringUTFChars(env, file_path, NULL);
    if (path == NULL) return;
    p2pshare_controller_start_send_wait((uint64_t) handle, path);
    (*env)->ReleaseStringUTFChars(env, file_path, path);
}

static void native_start_send_to_ticket(
    JNIEnv *env,
    jclass clazz,
    jlong handle,
    jstring file_path,
    jstring ticket
) {
    (void) clazz;
    if (file_path == NULL || ticket == NULL) return;

    const char *path = (*env)->GetStringUTFChars(env, file_path, NULL);
    if (path == NULL) return;

    const char *ticket_str = (*env)->GetStringUTFChars(env, ticket, NULL);
    if (ticket_str == NULL) {
        (*env)->ReleaseStringUTFChars(env, file_path, path);
        return;
    }

    p2pshare_controller_start_send_to_ticket((uint64_t) handle, path, ticket_str);

    (*env)->ReleaseStringUTFChars(env, ticket, ticket_str);
    (*env)->ReleaseStringUTFChars(env, file_path, path);
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

static jstring native_poll_event(JNIEnv *env, jclass clazz, jlong handle) {
    (void) clazz;
    const char *json = p2pshare_controller_poll_event_json((uint64_t) handle);
    if (json == NULL) {
        return NULL;
    }

    jstring out = (*env)->NewStringUTF(env, json);
    p2pshare_free_cstring(json);
    return out;
}

static void native_cancel(JNIEnv *env, jclass clazz, jlong handle) {
    (void) env;
    (void) clazz;
    p2pshare_controller_cancel((uint64_t) handle);
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
        {"nativeCreateController", "()J", (void *) native_create_controller},
        {"nativeStartSendWait", "(JLjava/lang/String;)V", (void *) native_start_send_wait},
        {"nativeStartSendToTicket", "(JLjava/lang/String;Ljava/lang/String;)V", (void *) native_start_send_to_ticket},
        {"nativeStartReceiveTarget", "(JLjava/lang/String;Ljava/lang/String;)V", (void *) native_start_receive_target},
        {"nativeStartReceiveListen", "(JLjava/lang/String;)V", (void *) native_start_receive_listen},
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
