# DBX Desktop

**English** | [中文](README.md)

DBX Desktop is a lightweight launcher for the `dbx` command. At startup it checks `http://127.0.0.1:4224`; if dbx is not already available, it runs `dbx` from the local PATH and waits for the service to become ready.

## Features

- Finds `dbx` in PATH and common global installation locations.
- Runs `dbx` directly; its default port is **4224**.
- Opens `http://127.0.0.1:4224` inside the app as soon as it is ready.
- Stops only the dbx process launched by this app when the app exits.
- Mobile builds can connect to a LAN dbx service, defaulting to port 4224.

## Development

Make sure `dbx` is available from your terminal, then run:

```bash
npm install
npm run tauri dev
```

Build the current platform installer:

```bash
npm run tauri build
```

The output of dbx started by this application is written to `dbx.log` in the system app-log directory.

## License

[MIT License](LICENSE)
