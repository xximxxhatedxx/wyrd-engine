# wyrd-wasm

Sandboxed Wasmtime WebAssembly guest module host, capability-based permissions (`manifest.toml`), D-Bus introspection/subscription bridge, and `ModuleSupervisor` for the Wyrd ecosystem.

## Build Requirements

Requires **Rust 1.96+** (Wasmtime 49.x declares `rust-version = 1.96`) and standard C build tooling (`base-devel` / `build-essential` / `gcc`) for compiling Wasmtime from source.

## Module Levels

Only `Wasm` modules are executed in 0.1.0. A `Wasm` module is a `.wasm`
binary compiled for the Wasmtime sandbox and receives capability-checked
host calls (`fs:read`, `dbus:call`, `socket:connect`, timers, path
watches, and so on) as described in the Capabilities section below.

`Native` and `Lua` are planned levels for future releases. They are
accepted by the manifest parser so existing manifests remain forward
compatible, but `ModuleSupervisor` does not spawn or execute them in
0.1.0; a `Native` or `Lua` manifest entry is skipped with a debug log.
`Process`-level modules are not supported in 0.1.0 and are planned for
0.2.0.

Spawning a module with a name that is already running stops the previous
instance before starting the replacement.

## Capabilities

- `fs:read:<path>` — read under an absolute path prefix; `fs:read:applications`
  grants reads only under nonempty absolute XDG application-data directories.
  `fs:read`, `fs:read:*`, and `*` grant unrestricted reads.
- `fs:watch:runtime`, `fs:watch:tmp`, `fs:watch:<path>` — watch runtime,
  temporary, or explicitly scoped paths. Bare `fs:watch` is a deprecated alias
  for `fs:watch:runtime`.
- `fs:write` / `fs:write:<path>` — rejected in 0.1.0; `host_write_file` always
  returns `-403`, including when the manifest is constructed directly.
- `process:spawn:<bin>` — spawn a named binary found through `$PATH`;
  `process:spawn:*` allows any binary name.
- `dbus:call:<bus>:<service>` — call one service; `dbus:call:<bus>:*`,
  `dbus:call:*`, and `dbus:call` grant broader call access.
- `dbus:subscribe:<bus>:<service>:<interface>` — subscribe to one interface.
  `dbus:subscribe:<bus>:<service>:*`, `dbus:subscribe:<bus>:*`,
  `dbus:subscribe:*`, and `dbus:subscribe` grant broader subscription access.
- `socket:connect:<name>` — connect to a named Unix socket;
  `socket:connect:*` allows any socket. `socket:connect`, `socket:*`, and
  `socket` are equivalent broad grants. Relative socket names require
  `XDG_RUNTIME_DIR` to be set to an absolute directory; there is no `/tmp`
  fallback.
- `env:read:<key>` — read the named environment variable;
  `env:read:*` grants all environment variables. `HOME`, XDG paths,
  `TERM`, `TERMINAL`, `LANG`, `LC_ALL`, and `WYRD_*` variables are allowed by the
  host's standard environment policy.
- `clipboard:read`, `clipboard:write` — read or write clipboard data;
  `clipboard:*` and `clipboard` grant both.
- `focus:request` — request focus; `focus:*`, `focus`, and `popups` also grant
  focus requests.
- `network:fetch:<host>` — fetch HTTP(S) image URLs from the named host.
  `network:fetch`, `network:fetch:*`, and `net:*` allow any host.
- `*` grants all supported capabilities except filesystem writes.

Granting `process:spawn:sh`, `process:spawn:bash`, `process:spawn:python`,
`process:spawn:python3`, `process:spawn:env`, `process:spawn:perl`, `process:spawn:ruby`,
`process:spawn:node`, or `process:spawn:*` is effectively equivalent to
granting arbitrary code execution. Grant these only with explicit user consent.

`network:fetch` gates `host_image_cached`. The renderer never fetches HTTP(S)
URLs supplied through widget updates; module authors must call
`host_image_cached` to trigger a network fetch.

## Module lookup

Manifests are loaded only from absolute directories: `$XDG_CONFIG_HOME/wyrd/modules`,
`/usr/lib/wyrd/modules`, and `/usr/share/wyrd/modules` (with the standard
`$HOME/.config` fallback). A developer may set `WYRD_MODULE_MANIFEST_DIR` to an
absolute directory to load manifests from a checkout.

Module binaries are loaded only from absolute user or system module directories:
`$XDG_DATA_HOME/wyrd/modules`, `$XDG_CONFIG_HOME/wyrd/modules`,
`/usr/share/wyrd/modules`, and `/usr/lib/wyrd/modules` (with standard home-directory
fallbacks for the XDG paths). Developers may set `WYRD_MODULES_DIR` to an absolute
directory to load a build from a checkout. Relative paths are never searched; the
current working directory is never searched for manifests or binaries.

Part of the [`wyrd-engine`](https://github.com/xximxxhatedxx/wyrd-engine) workspace.

## Known Limitations

- Seat, pointer, and keyboard capability changes create and release Wayland
  proxies dynamically, but live hot-plug is not covered by automated tests;
  manually test it before release.
- Filesystem capability checks canonicalize paths before the later file or
  watcher operation, leaving a TOCTOU window. `O_NOFOLLOW` protects the final
  file component for reads, but not every parent-component race. An
  `openat2(RESOLVE_BENEATH)`-based implementation is planned.
- `.wasm` modules are compiled at runtime; there is no AOT compilation in
  0.1.0. The disk `.cwasm` cache was removed because deserializing guest-adjacent
  native code is unsafe.
- `EpochTicker` currently uses one thread per running module. A shared
  `WasmEngine` ticker is a 0.1.1 optimization.

## License

Dual-licensed under [MIT](https://github.com/xximxxhatedxx/wyrd-engine/blob/main/LICENSE-MIT) or [Apache-2.0](https://github.com/xximxxhatedxx/wyrd-engine/blob/main/LICENSE-APACHE).
