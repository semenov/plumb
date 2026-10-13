// storage on wasm-server (`--target wasm-server`): the project's files,
// kept by the host (platform:sdk/storage). std/storage.plumb calls these
// on wasm and keeps files in a directory otherwise. Details come back as
// JSON, which the Plumb side decodes.

static guest_string_t lt_st_str(lt_text *t) { return (guest_string_t){ (uint8_t *)t->data, (size_t)t->len }; }

static lt_err lt_st_fail(guest_string_t *err) {
    char buf[700];
    snprintf(buf, sizeof buf, "storage: %.*s", (int)(err->len > 600 ? 600 : err->len), (const char *)err->ptr);
    guest_string_free(err);
    return lt_make_failure(lt_text_cstr(buf));
}

static lt_err lt_storage_put(lt_text *key, lt_bytes *data, lt_text *content_type, bool public_) {
    guest_string_t k = lt_st_str(key), ct = lt_st_str(content_type), err;
    guest_list_u8_t d = { data->data, (size_t)data->len };
    if (!platform_sdk_storage_put(&k, &d, &ct, public_, &err)) return lt_st_fail(&err);
    return (lt_err){ 0 };
}

// the file's content; a failure if there is none (the Plumb side asks
// `info` first)
static lt_err lt_storage_get(lt_text *key, lt_bytes **out) {
    guest_string_t k = lt_st_str(key), err;
    guest_option_list_u8_t r;
    if (!platform_sdk_storage_get(&k, &r, &err)) return lt_st_fail(&err);
    if (!r.is_some) return lt_make_failure(lt_text_cstr("storage: the file is gone"));
    *out = lt_bytes_from(r.val.ptr, (int64_t)r.val.len);
    guest_option_list_u8_free(&r);
    return (lt_err){ 0 };
}

// a growing buffer for the JSON
typedef struct {
    char *p;
    size_t n, cap;
} lt_st_buf;

static void lt_st_add(lt_st_buf *b, const char *s) {
    size_t k = strlen(s);
    if (b->n + k + 1 > b->cap) {
        b->cap = (b->n + k + 1) * 2;
        b->p = (char *)realloc(b->p, b->cap);
    }
    memcpy(b->p + b->n, s, k + 1);
    b->n += k;
}

static lt_text *lt_st_done(lt_st_buf *b) {
    lt_text *t = lt_text_from(b->p ? b->p : "", (int64_t)b->n);
    free(b->p);
    return t;
}

static void lt_st_json_str(lt_st_buf *b, const uint8_t *s, size_t n) {
    lt_st_add(b, "\"");
    for (size_t i = 0; i < n; i++) {
        char esc[8];
        if (s[i] == '"' || s[i] == '\\') {
            esc[0] = '\\', esc[1] = (char)s[i], esc[2] = 0;
        } else if (s[i] < 0x20) {
            snprintf(esc, sizeof esc, "\\u%04x", s[i]);
        } else {
            esc[0] = (char)s[i], esc[1] = 0;
        }
        lt_st_add(b, esc);
    }
    lt_st_add(b, "\"");
}

static void lt_st_json_object(lt_st_buf *b, platform_sdk_storage_object_t *o) {
    char num[160];
    lt_st_add(b, "{\"key\":");
    lt_st_json_str(b, o->key.ptr, o->key.len);
    lt_st_add(b, ",\"content_type\":");
    lt_st_json_str(b, o->content_type.ptr, o->content_type.len);
    snprintf(num, sizeof num, ",\"size\":%llu,\"public\":%s,\"modified_ms\":%llu}", (unsigned long long)o->size, o->public_ ? "true" : "false", (unsigned long long)o->modified_ms);
    lt_st_add(b, num);
}

// the file's details as JSON, or "" if there is no such file
static lt_err lt_storage_info(lt_text *key, lt_text **out) {
    guest_string_t k = lt_st_str(key), err;
    platform_sdk_storage_option_object_t r;
    if (!platform_sdk_storage_info(&k, &r, &err)) return lt_st_fail(&err);
    lt_st_buf b = { 0 };
    if (r.is_some) lt_st_json_object(&b, &r.val);
    platform_sdk_storage_option_object_free(&r);
    *out = lt_st_done(&b);
    return (lt_err){ 0 };
}

static lt_err lt_storage_delete(lt_text *key, bool *out) {
    guest_string_t k = lt_st_str(key), err;
    if (!platform_sdk_storage_delete(&k, out, &err)) return lt_st_fail(&err);
    return (lt_err){ 0 };
}

// a JSON array of details
static lt_err lt_storage_list(lt_text *prefix, lt_text *after, int64_t limit, lt_text **out) {
    guest_string_t p = lt_st_str(prefix), a = lt_st_str(after), err;
    platform_sdk_storage_list_object_t r;
    if (!platform_sdk_storage_list(&p, &a, (uint32_t)(limit < 0 ? 0 : limit > 100000 ? 100000 : limit), &r, &err)) return lt_st_fail(&err);
    lt_st_buf b = { 0 };
    lt_st_add(&b, "[");
    for (size_t i = 0; i < r.len; i++) {
        if (i) lt_st_add(&b, ",");
        lt_st_json_object(&b, &r.ptr[i]);
    }
    lt_st_add(&b, "]");
    platform_sdk_storage_list_object_free(&r);
    *out = lt_st_done(&b);
    return (lt_err){ 0 };
}

static lt_err lt_st_text_result(bool ok, guest_string_t *ret, guest_string_t *err, lt_text **out) {
    if (!ok) return lt_st_fail(err);
    *out = lt_text_from((const char *)ret->ptr, (int64_t)ret->len);
    guest_string_free(ret);
    return (lt_err){ 0 };
}

static lt_err lt_storage_url(lt_text *key, int64_t ttl_ms, lt_text **out) {
    guest_string_t k = lt_st_str(key), ret, err;
    bool ok = platform_sdk_storage_url(&k, (uint64_t)(ttl_ms < 0 ? 0 : ttl_ms), &ret, &err);
    return lt_st_text_result(ok, &ret, &err, out);
}

static lt_err lt_storage_upload_url(lt_text *key, int64_t max_bytes, lt_text *content_type, bool public_, int64_t ttl_ms, lt_text **out) {
    guest_string_t k = lt_st_str(key), ct = lt_st_str(content_type), ret, err;
    bool ok = platform_sdk_storage_upload_url(&k, (uint64_t)(max_bytes < 0 ? 0 : max_bytes), &ct, public_, (uint64_t)(ttl_ms < 0 ? 0 : ttl_ms), &ret, &err);
    return lt_st_text_result(ok, &ret, &err, out);
}
