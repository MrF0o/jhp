(function () {
    const g = (typeof global !== 'undefined') ? global : (typeof globalThis !== 'undefined' ? globalThis : this);
    if (typeof g.Buffer === 'function') return g.Buffer;
    function utf8Encode(str) {
        const out = []; let i = 0; while (i < str.length) {
            let cp = str.codePointAt(i); i += cp > 0xFFFF ? 2 : 1;
            if (cp <= 0x7F) { out.push(cp); }
            else if (cp <= 0x7FF) { out.push(0xC0 | (cp >> 6)); out.push(0x80 | (cp & 0x3F)); }
            else if (cp <= 0xFFFF) { out.push(0xE0 | (cp >> 12)); out.push(0x80 | ((cp >> 6) & 0x3F)); out.push(0x80 | (cp & 0x3F)); }
            else { out.push(0xF0 | (cp >> 18)); out.push(0x80 | ((cp >> 12) & 0x3F)); out.push(0x80 | ((cp >> 6) & 0x3F)); out.push(0x80 | (cp & 0x3F)); }
        } return new Uint8Array(out);
    }
    function utf8Decode(bytes) {
        let out = ''; for (let i = 0; i < bytes.length;) {
            const b1 = bytes[i++];
            if ((b1 & 0x80) === 0) { out += String.fromCodePoint(b1); }
            else if ((b1 & 0xE0) === 0xC0) { const b2 = bytes[i++] & 0x3F; out += String.fromCodePoint(((b1 & 0x1F) << 6) | b2); }
            else if ((b1 & 0xF0) === 0xE0) { const b2 = bytes[i++] & 0x3F, b3 = bytes[i++] & 0x3F; out += String.fromCodePoint(((b1 & 0x0F) << 12) | (b2 << 6) | b3); }
            else { const b2 = bytes[i++] & 0x3F, b3 = bytes[i++] & 0x3F, b4 = bytes[i++] & 0x3F; out += String.fromCodePoint(((b1 & 0x07) << 18) | (b2 << 12) | (b3 << 6) | b4); }
        } return out;
    }
    const B64CHARS = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';
    function b64encode(bytes) {
        let out = ''; let i = 0; for (; i + 2 < bytes.length; i += 3) { const n = (bytes[i] << 16) | (bytes[i + 1] << 8) | bytes[i + 2]; out += B64CHARS[(n >> 18) & 63] + B64CHARS[(n >> 12) & 63] + B64CHARS[(n >> 6) & 63] + B64CHARS[n & 63]; }
        const rem = bytes.length - i; if (rem === 1) { const n = (bytes[i] << 16); out += B64CHARS[(n >> 18) & 63] + B64CHARS[(n >> 12) & 63] + '=='; }
        else if (rem === 2) { const n = (bytes[i] << 16) | (bytes[i + 1] << 8); out += B64CHARS[(n >> 18) & 63] + B64CHARS[(n >> 12) & 63] + B64CHARS[(n >> 6) & 63] + '='; }
        return out;
    }
    const B64REV = (() => { const r = new Int16Array(256); for (let i = 0; i < 256; i++) r[i] = -1; for (let i = 0; i < B64CHARS.length; i++) r[B64CHARS.charCodeAt(i)] = i; return r; })();
    function b64decode(str) {
        const clean = String(str).replace(/\s+/g, ''); const len = clean.length; if (len % 4 === 1) throw new Error('Invalid base64');
        let pad = 0; if (len > 0 && clean[len - 1] === '=') { pad++; if (clean[len - 2] === '=') pad++; }
        const outLen = Math.floor((len * 3) / 4) - pad; const out = new Uint8Array(outLen); let oi = 0;
        for (let i = 0; i < len; i += 4) {
            const c0 = B64REV[clean.charCodeAt(i)], c1 = B64REV[clean.charCodeAt(i + 1)];
            const c2 = clean[i + 2] === '=' ? 0 : B64REV[clean.charCodeAt(i + 2)], c3 = clean[i + 3] === '=' ? 0 : B64REV[clean.charCodeAt(i + 3)];
            const n = (c0 << 18) | (c1 << 12) | ((c2 & 63) << 6) | (c3 & 63);
            if (oi < outLen) out[oi++] = (n >> 16) & 255; if (oi < outLen) out[oi++] = (n >> 8) & 255; if (oi < outLen) out[oi++] = n & 255;
        } return out;
    }
    function BufferImpl(u8) { const arr = (u8 instanceof Uint8Array) ? u8 : (u8 instanceof ArrayBuffer ? new Uint8Array(u8) : new Uint8Array(0)); Object.setPrototypeOf(arr, Buffer.prototype); return arr; }
    function Buffer(u8) { if (!(this instanceof Buffer)) return BufferImpl(u8); return BufferImpl(u8); }
    Buffer.from = function (input, encoding) {
        if (input instanceof Uint8Array) return BufferImpl(input);
        if (input instanceof ArrayBuffer) return BufferImpl(new Uint8Array(input));
        if (typeof input === 'string') {
            if (!encoding || encoding === 'utf8') return BufferImpl(utf8Encode(input));
            if (encoding === 'base64') return BufferImpl(b64decode(input));
            if (encoding === 'binary') { const out = new Uint8Array(input.length); for (let i = 0; i < input.length; i++) out[i] = input.charCodeAt(i) & 255; return BufferImpl(out); }
        }
        throw new Error('Unsupported Buffer.from input');
    };
    Buffer.prototype.toString = function (encoding) {
        encoding = encoding || 'utf8'; const b = this;
        if (encoding === 'utf8') return utf8Decode(b);
        if (encoding === 'base64') return b64encode(b);
        if (encoding === 'binary') { let s = ''; for (let i = 0; i < b.length; i++) s += String.fromCharCode(b[i]); return s; }
        throw new Error('Unsupported encoding');
    };
    g.Buffer = Buffer;
    return Buffer;
})();
