// db on wasm-server (`--target wasm-server`): the host holds the databases
// (SQLite or Postgres) and the app reaches them through platform:sdk/sql.
// `db.open(name)` takes the name of a database granted to the app, not a
// file. The same functions as db.h, so the generated code doesn't change.

typedef struct {
    lt_handle h;
    lt_text *name;
    bool closed;
    // the open transaction; deeper ones are savepoints
    bool in_tx;
    platform_sdk_sql_own_transaction_t tx;
    int depth;
    int64_t last_id;
} lt_db;

typedef struct {
    int kind; // 0 null, 1 int, 2 real, 3 text, 4 blob
    int64_t i;
    double f;
    const char *s;
    int64_t len;
} lt_dbval;

static void lt_db_free(lt_handle *h) {
    lt_db *d = (lt_db *)h;
    if (d->in_tx) platform_sdk_sql_transaction_drop_own(d->tx); // rolls back
    lt_text_drop(d->name);
    free(d);
}

static guest_string_t lt_db_str(const char *s, int64_t n) { return (guest_string_t){ (uint8_t *)s, (size_t)n }; }

static lt_err lt_db_fail(platform_sdk_sql_error_t *err, const char *what, lt_text *sql) {
    const char *kind = "";
    guest_string_t *s = NULL;
    char extra[64] = "";
    switch (err->tag) {
    case PLATFORM_SDK_SQL_ERROR_NOT_ALLOWED: kind = "not allowed: "; s = &err->val.not_allowed; break;
    case PLATFORM_SDK_SQL_ERROR_UNAVAILABLE: kind = "unavailable: "; s = &err->val.unavailable; break;
    case PLATFORM_SDK_SQL_ERROR_QUERY: s = &err->val.query; break;
    case PLATFORM_SDK_SQL_ERROR_CONVERSION: kind = "can't convert a value: "; s = &err->val.conversion; break;
    case PLATFORM_SDK_SQL_ERROR_TIMEOUT: kind = "took too long"; break;
    default: snprintf(extra, sizeof extra, "more than %u rows", (unsigned)err->val.too_many_rows); break;
    }
    char buf[1024];
    snprintf(buf, sizeof buf, "db: %s: %s%.*s%s%s%.200s", what, kind, s ? (int)s->len : 0, s ? (const char *)s->ptr : "", extra, sql ? " (in: " : "", sql ? sql->data : "");
    size_t n = strlen(buf);
    if (sql && n + 1 < sizeof buf) buf[n] = ')', buf[n + 1] = 0;
    platform_sdk_sql_error_free(err);
    return lt_make_failure(lt_text_cstr(buf));
}

static lt_err lt_db_open(lt_text *name, lt_handle **out) {
    lt_db *d = (lt_db *)calloc(1, sizeof(lt_db));
    d->h.rc = 1;
    d->h.free = lt_db_free;
    d->name = name;
    lt_text_dup(name);
    *out = &d->h;
    return (lt_err){ 0 };
}

static platform_sdk_sql_list_value_t lt_db_params(const lt_dbval *vals, int64_t n) {
    platform_sdk_sql_value_t *v = (platform_sdk_sql_value_t *)calloc((size_t)(n ? n : 1), sizeof *v);
    for (int64_t i = 0; i < n; i++) {
        switch (vals[i].kind) {
        case 0: v[i].tag = PLATFORM_SDK_SQL_VALUE_NULL; break;
        case 1: v[i].tag = PLATFORM_SDK_SQL_VALUE_INT; v[i].val.int_ = vals[i].i; break;
        case 2: v[i].tag = PLATFORM_SDK_SQL_VALUE_FLOAT; v[i].val.float_ = vals[i].f; break;
        case 3: v[i].tag = PLATFORM_SDK_SQL_VALUE_TEXT; v[i].val.text = lt_db_str(vals[i].s, vals[i].len); break;
        default: v[i].tag = PLATFORM_SDK_SQL_VALUE_BLOB; v[i].val.blob = (guest_list_u8_t){ (uint8_t *)vals[i].s, (size_t)vals[i].len }; break;
        }
    }
    return (platform_sdk_sql_list_value_t){ v, (size_t)n };
}

static lt_err lt_db_closed(lt_db *d) {
    if (d->closed) return lt_make_failure(lt_text_cstr("db: the connection is closed"));
    return (lt_err){ 0 };
}

// a statement: inside the open transaction, if there is one
static lt_err lt_db_exec(lt_db *d, const char *sql, int64_t len, const lt_dbval *vals, int64_t n, int64_t *changed, lt_text *shown) {
    platform_sdk_sql_list_value_t p = lt_db_params(vals, n);
    guest_string_t st = lt_db_str(sql, len);
    uint64_t count = 0;
    platform_sdk_sql_error_t err;
    bool ok;
    if (d->in_tx) {
        ok = platform_sdk_sql_method_transaction_execute(platform_sdk_sql_borrow_transaction(d->tx), &st, &p, &count, &err);
    } else {
        guest_string_t db = lt_db_str(d->name->data, d->name->len);
        ok = platform_sdk_sql_execute(&db, &st, &p, &count, &err);
    }
    free(p.ptr);
    if (!ok) return lt_db_fail(&err, "the statement failed", shown);
    if (changed) *changed = (int64_t)count;
    return (lt_err){ 0 };
}

static lt_err lt_db_rows(lt_db *d, const char *sql, int64_t len, const lt_dbval *vals, int64_t n, platform_sdk_sql_query_result_t *res, lt_text *shown) {
    platform_sdk_sql_list_value_t p = lt_db_params(vals, n);
    guest_string_t st = lt_db_str(sql, len);
    platform_sdk_sql_error_t err;
    bool ok;
    if (d->in_tx) {
        ok = platform_sdk_sql_method_transaction_query(platform_sdk_sql_borrow_transaction(d->tx), &st, &p, res, &err);
    } else {
        guest_string_t db = lt_db_str(d->name->data, d->name->len);
        ok = platform_sdk_sql_query(&db, &st, &p, res, &err);
    }
    free(p.ptr);
    if (!ok) return lt_db_fail(&err, "the query failed", shown);
    return (lt_err){ 0 };
}

static lt_err lt_db_execute(lt_handle *h, lt_text *sql, const lt_dbval *vals, int64_t n, int64_t *changed) {
    lt_db *d = (lt_db *)h;
    lt_err e = lt_db_closed(d);
    if (e.obj) return e;
    return lt_db_exec(d, sql->data, sql->len, vals, n, changed, sql);
}

static lt_err lt_db_begin(lt_handle *h);
static lt_err lt_db_end(lt_handle *h, bool commit);

// db.Connection.insert: the host's connections are pooled, so the insert
// and the question for its id go in one transaction (one connection).
// SQLite knows last_insert_rowid(), Postgres lastval().
static lt_err lt_db_insert(lt_handle *h, lt_text *sql, const lt_dbval *vals, int64_t n, int64_t *id) {
    lt_db *d = (lt_db *)h;
    lt_err e = lt_db_begin(h);
    if (e.obj) return e;
    e = lt_db_exec(d, sql->data, sql->len, vals, n, NULL, sql);
    platform_sdk_sql_query_result_t res;
    if (!e.obj) {
        static const char *ask[2] = { "select last_insert_rowid()", "select lastval()" };
        for (int i = 0; i < 2; i++) {
            e = lt_db_rows(d, ask[i], (int64_t)strlen(ask[i]), NULL, 0, &res, NULL);
            if (!e.obj) break;
            if (i == 0) lt_iface_drop(e);
        }
    }
    if (e.obj) {
        lt_err e2 = lt_db_end(h, false);
        lt_iface_drop(e2);
        return e;
    }
    int64_t got = 0;
    if (res.rows.len > 0 && res.rows.ptr[0].len > 0 && res.rows.ptr[0].ptr[0].tag == PLATFORM_SDK_SQL_VALUE_INT) got = res.rows.ptr[0].ptr[0].val.int_;
    platform_sdk_sql_query_result_free(&res);
    e = lt_db_end(h, true);
    if (e.obj) return e;
    d->last_id = got;
    *id = got;
    return (lt_err){ 0 };
}

// runs a query and calls `row` with each row as an object
static lt_err lt_db_query(lt_handle *h, lt_text *sql, const lt_dbval *vals, int64_t n, lt_err (*row)(const lt_dyn *, void *), void *ctx) {
    lt_db *d = (lt_db *)h;
    lt_err e = lt_db_closed(d);
    if (e.obj) return e;
    platform_sdk_sql_query_result_t res;
    e = lt_db_rows(d, sql->data, sql->len, vals, n, &res, sql);
    if (e.obj) return e;
    int64_t cols = (int64_t)res.columns.len;
    lt_arena ar = { 0 };
    lt_dyn o;
    memset(&o, 0, sizeof o);
    o.kind = LT_D_OBJ;
    o.n = cols;
    o.items = (lt_dyn *)lt_arena_alloc(&ar, sizeof(lt_dyn) * (size_t)(cols ? cols : 1));
    o.keys = (const char **)lt_arena_alloc(&ar, sizeof(char *) * (size_t)(cols ? cols : 1));
    o.klens = (int64_t *)lt_arena_alloc(&ar, sizeof(int64_t) * (size_t)(cols ? cols : 1));
    for (int64_t c = 0; c < cols; c++) {
        o.keys[c] = (const char *)res.columns.ptr[c].ptr;
        o.klens[c] = (int64_t)res.columns.ptr[c].len;
    }
    for (size_t r = 0; r < res.rows.len && !e.obj; r++) {
        platform_sdk_sql_row_t *rw = &res.rows.ptr[r];
        for (int64_t c = 0; c < cols; c++) {
            lt_dyn *v = &o.items[c];
            memset(v, 0, sizeof *v);
            platform_sdk_sql_value_t *x = (size_t)c < rw->len ? &rw->ptr[c] : NULL;
            switch (x ? x->tag : PLATFORM_SDK_SQL_VALUE_NULL) {
            case PLATFORM_SDK_SQL_VALUE_INT:
                v->kind = LT_D_NUM;
                v->is_int = true;
                v->i = x->val.int_;
                v->num = (double)v->i;
                break;
            case PLATFORM_SDK_SQL_VALUE_FLOAT:
                v->kind = LT_D_NUM;
                v->num = x->val.float_;
                break;
            case PLATFORM_SDK_SQL_VALUE_BOOLEAN:
                v->kind = LT_D_BOOL;
                v->b = x->val.boolean;
                break;
            case PLATFORM_SDK_SQL_VALUE_TEXT:
                v->kind = LT_D_STR;
                v->s = (const char *)x->val.text.ptr;
                v->slen = (int64_t)x->val.text.len;
                break;
            case PLATFORM_SDK_SQL_VALUE_BLOB:
                v->kind = LT_D_STR;
                v->raw = true;
                v->s = (const char *)x->val.blob.ptr;
                v->slen = (int64_t)x->val.blob.len;
                break;
            default:
                v->kind = LT_D_NULL;
            }
        }
        e = row(&o, ctx);
    }
    lt_arena_free(&ar);
    platform_sdk_sql_query_result_free(&res);
    return e;
}

static int64_t lt_db_last_id(lt_handle *h) {
    return ((lt_db *)h)->last_id;
}

static lt_err lt_db_close(lt_handle *h) {
    lt_db *d = (lt_db *)h;
    d->closed = true;
    return (lt_err){ 0 };
}

// db.Connection.transaction: one connection of the host's until lt_db_end;
// inside it, another transaction is a savepoint.
static lt_err lt_db_begin(lt_handle *h) {
    lt_db *d = (lt_db *)h;
    lt_err e = lt_db_closed(d);
    if (e.obj) return e;
    if (!d->in_tx) {
        guest_string_t db = lt_db_str(d->name->data, d->name->len);
        platform_sdk_sql_error_t err;
        if (!platform_sdk_sql_static_transaction_begin(&db, &d->tx, &err)) return lt_db_fail(&err, "can't start a transaction", NULL);
        d->in_tx = true;
        d->depth = 1;
        return (lt_err){ 0 };
    }
    char sql[64];
    snprintf(sql, sizeof sql, "savepoint lt_%d", d->depth);
    e = lt_db_exec(d, sql, (int64_t)strlen(sql), NULL, 0, NULL, NULL);
    if (!e.obj) d->depth++;
    return e;
}

static lt_err lt_db_end(lt_handle *h, bool commit) {
    lt_db *d = (lt_db *)h;
    if (!d->in_tx) return lt_make_failure(lt_text_cstr("db: no transaction to end"));
    int depth = --d->depth;
    if (depth == 0) {
        d->in_tx = false;
        if (!commit) {
            platform_sdk_sql_transaction_drop_own(d->tx); // rolls back
            return (lt_err){ 0 };
        }
        platform_sdk_sql_error_t err;
        if (!platform_sdk_sql_static_transaction_commit(d->tx, &err)) return lt_db_fail(&err, "can't commit", NULL);
        return (lt_err){ 0 };
    }
    char sql[80];
    if (commit) snprintf(sql, sizeof sql, "release lt_%d", depth);
    else snprintf(sql, sizeof sql, "rollback to lt_%d", depth);
    lt_err e = lt_db_exec(d, sql, (int64_t)strlen(sql), NULL, 0, NULL, NULL);
    if (!e.obj && !commit) {
        snprintf(sql, sizeof sql, "release lt_%d", depth);
        e = lt_db_exec(d, sql, (int64_t)strlen(sql), NULL, 0, NULL, NULL);
    }
    return e;
}
