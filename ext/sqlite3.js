// Sqlite3 module shim
// When included via include('sqlite3'), this file exposes a Sqlite3 namespace
// that forwards to native extension functions exported by libjhp_ext_sqlite.so.
//
// Available native functions are bound onto global by the engine. Here we
// assemble a higher-level module facade.

// Ensure global alias exists (engine also sets it, but keep it robust)
if (typeof global === 'undefined') {
    this.global = this;
}

const Sqlite3 = {};

if (typeof global.sqlite_test === 'function') {
    Sqlite3.test = (...args) => global.sqlite_test(...args);
}
global.Sqlite3 = Sqlite3;
