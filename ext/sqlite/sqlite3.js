(function () {
    const g = (typeof global !== 'undefined') ? global : this;
    // Ensure Buffer is available via extension system (idempotent)
    try { if (typeof g.Buffer === 'undefined') { include('buffer'); } } catch (_) {}
    const Sqlite3 = g.Sqlite3 || {};

    // Keep existing test method if present on module object or global
    if (typeof Sqlite3.sqlite_test === 'function') {
        Sqlite3.test = (...args) => Sqlite3.sqlite_test(...args);
    } else if (typeof g.sqlite_test === 'function') {
        Sqlite3.test = (...args) => g.sqlite_test(...args);
    }

    function ensure(obj, name) {
        const fn = obj && typeof obj[name] === 'function' ? obj[name] : undefined;
        if (typeof fn !== 'function') throw new Error(`Native function ${name} not found`);
        return fn;
    }

    const nativeSource = (g.Sqlite3 && typeof g.Sqlite3 === 'object') ? g.Sqlite3 : g;
    const _open = ensure(nativeSource, 'sqlite_open');
    const _close = ensure(nativeSource, 'sqlite_close');
    const _exec = ensure(nativeSource, 'sqlite_execute');
    const _query = ensure(nativeSource, 'sqlite_query');
    const _version = ensure(nativeSource, 'sqlite_version');
    const _changes = ensure(nativeSource, 'sqlite_changes');
    const _lastid = ensure(nativeSource, 'sqlite_last_insert_rowid');

    function unwrap(res) {
        // Native returns JSON objects; on errors we standardize to { error, code }
        if (res && typeof res === 'object' && 'error' in res) {
            const err = new Error(res.error || 'Sqlite3 error');
            if (res.code != null) err.code = res.code;
            throw err;
        }
        return res;
    }

    function utf8Encode(str) {
        const out = [];
        let i = 0;
        while (i < str.length) {
            let code = str.codePointAt(i);
            if (code > 0xFFFF) i += 2; else i += 1;
            if (code <= 0x7F) {
                out.push(code);
            } else if (code <= 0x7FF) {
                out.push(0xC0 | (code >> 6));
                out.push(0x80 | (code & 0x3F));
            } else if (code <= 0xFFFF) {
                out.push(0xE0 | (code >> 12));
                out.push(0x80 | ((code >> 6) & 0x3F));
                out.push(0x80 | (code & 0x3F));
            } else {
                out.push(0xF0 | (code >> 18));
                out.push(0x80 | ((code >> 12) & 0x3F));
                out.push(0x80 | ((code >> 6) & 0x3F));
                out.push(0x80 | (code & 0x3F));
            }
        }
        return new Uint8Array(out);
    }
    function utf8Decode(bytes) {
        let out = '';
        for (let i = 0; i < bytes.length;) {
            const b1 = bytes[i++];
            if ((b1 & 0x80) === 0) {
                out += String.fromCodePoint(b1);
            } else if ((b1 & 0xE0) === 0xC0) {
                const b2 = bytes[i++] & 0x3F;
                const cp = ((b1 & 0x1F) << 6) | b2;
                out += String.fromCodePoint(cp);
            } else if ((b1 & 0xF0) === 0xE0) {
                const b2 = bytes[i++] & 0x3F;
                const b3 = bytes[i++] & 0x3F;
                const cp = ((b1 & 0x0F) << 12) | (b2 << 6) | b3;
                out += String.fromCodePoint(cp);
            } else {
                const b2 = bytes[i++] & 0x3F;
                const b3 = bytes[i++] & 0x3F;
                const b4 = bytes[i++] & 0x3F;
                const cp = ((b1 & 0x07) << 18) | (b2 << 12) | (b3 << 6) | b4;
                out += String.fromCodePoint(cp);
            }
        }
        return out;
    }
    function bytesToBase64(bytes) {
        return Buffer.from(bytes).toString('base64');
    }
    function base64ToBytes(b64) {
        return new Uint8Array(Buffer.from(b64, 'base64'));
    }

    Sqlite3.blob = function (value, encoding = 'bytes') {
        if (value == null) return null;
        if (encoding === 'bytes') {
            if (value instanceof Uint8Array) {
                return { blob: bytesToBase64(value), length: value.length };
            }
            if (value instanceof ArrayBuffer) {
                const u8 = new Uint8Array(value);
                return { blob: bytesToBase64(u8), length: u8.length };
            }
            throw new Error('Sqlite3.blob: expected Uint8Array or ArrayBuffer for encoding=bytes');
        } else if (encoding === 'base64') {
            if (typeof value !== 'string') throw new Error('Sqlite3.blob: base64 value must be a string');
            return { blob: value, length: base64ToBytes(value).length };
        } else if (encoding === 'utf8') {
            if (typeof value !== 'string') throw new Error('Sqlite3.blob: utf8 value must be a string');
            // encode utf8 string to bytes (no TextEncoder as of yet)
            const u8 = utf8Encode(value);
            return { blob: bytesToBase64(u8), length: u8.length };
        }
        throw new Error('Sqlite3.blob: unsupported encoding');
    };

    Sqlite3.toBytes = function (blobObj) {
        if (!blobObj || typeof blobObj !== 'object' || typeof blobObj.blob !== 'string') {
            throw new Error('Sqlite3.toBytes: invalid blob object');
        }
        return base64ToBytes(blobObj.blob);
    };

    Sqlite3.toText = function (blobObj, encoding = 'utf8') {
        const bytes = Sqlite3.toBytes(blobObj);
        if (encoding === 'utf8') {
            return utf8Decode(bytes);
        }
        throw new Error('Sqlite3.toText: unsupported encoding');
    };

    class Database {
        constructor(handle) {
            this.handle = handle;
        }
        get changes() { return unwrap(_changes(this.handle)).changes; }
        get lastInsertRowId() { return unwrap(_lastid(this.handle)).id; }
        exec(sql, params) {
            return unwrap(_exec(this.handle, String(sql), params));
        }
        query(sql, params, opts) {
            return unwrap(_query(this.handle, String(sql), params, opts));
        }
        pragma(name, value) {
            const sql = value === undefined ? `PRAGMA ${name}` : `PRAGMA ${name}=${value}`;
            return this.query(sql);
        }
        transaction(fn) {
            unwrap(_exec(this.handle, 'BEGIN'));
            try {
                const res = fn(this);
                unwrap(_exec(this.handle, 'COMMIT'));
                return res;
            } catch (e) {
                try { unwrap(_exec(this.handle, 'ROLLBACK')); } catch (_) { }
                throw e;
            }
        }
        close() {
            const res = unwrap(_close(this.handle));
            this.handle = 0;
            return res;
        }
    }

    Sqlite3.open = function (path, opts) {
        const res = unwrap(_open(String(path), opts));
        return new Database(res.db);
    };
    Sqlite3.version = function () { return unwrap(_version()).version; };
    Sqlite3.Database = Database;

    g.Sqlite3 = Sqlite3;
    return Sqlite3;
})();
