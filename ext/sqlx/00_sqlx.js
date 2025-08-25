// SQLX JS extension shim.
// Wraps native functions if present

(function (global) {
    if (global.sqlx && global.sqlx.__jhpSqlxShim) return; // idempotent

    const native = {
        connect: typeof global.sqlx_connect === 'function' ? global.sqlx_connect : null,
        query: typeof global.sqlx_query === 'function' ? global.sqlx_query : null,
    };

    function connect(url) {
        if (native.connect) {
            const res = native.connect(url);
            if (res && res.type === 'connected') return { id: res.id };
            if (res && res.type === 'error') throw new Error(res.message || 'sqlx connect error');
        }
        throw new Error('sqlx connect: native extension not loaded');
    }

    function query(conn, sql, params) {
        if (native.query) {
            const res = native.query(conn, sql, params || []);
            if (res && res.type === 'query_result') {
                // rows are array-of-arrays in the same order as columns.
                return { columns: res.columns || [], rows: res.rows || [], rowCount: res.row_count | 0 };
            }
            if (res && res.type === 'error') throw new Error(res.message || 'sqlx query error');
        }
        throw new Error('sqlx query: native extension not loaded');
    }

    // fallback
    function sqlx() { return sqlx; }
    sqlx.connect = connect;
    sqlx.query = query;
    // marker and convenience
    sqlx.__jhpSqlxShim = true;
    sqlx.default = sqlx;

    global.sqlx = sqlx;
})(global);
