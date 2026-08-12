# Honoka Compositor and Window Manager

Honoka is Nanami's native compositing desktop and window service. It runs as a
user-space process and registers `honoka-service` through Alpha's service
registry.

## Responsibilities

- map and compose the framebuffer provided by `fb-server`;
- create and destroy client windows;
- share a logical framebuffer with each client;
- render rounded window frames, title bars, close buttons, shadows, and titles;
- alpha-compose translucent client content;
- track focus, z-order, dragging, and close requests;
- render the desktop wallpaper, top bar, clock, and cursor;
- consume input from `input-server`;
- load desktop configuration and themes from the ext2 root filesystem;
- process damage notifications without redrawing unchanged regions.

## Data Flow

```text
fb-server shared framebuffer
  <- Honoka compositor
       <- per-window shared logical framebuffer
       <- client damage notification

ps2-server -> input-server -> shared input queue -> Honoka
```

Clients specify drawable content dimensions. Window decorations are outside the
client framebuffer and remain compositor-owned.

## Themes and Assets

Source assets are stored under `assets`:

- `assets/themes/config` selects the theme;
- `assets/themes/*.theme` contains theme values;
- `assets/wallpapers` contains desktop backgrounds;
- `assets/fonts/hack.ttf` is the bundled UI font.

The rootfs image builder installs the configuration and theme files under
`/.honoka`. Themes are data files rather than hard-coded compositor palettes.

The Hack font license is preserved in [licenses/hack.md](licenses/hack.md).

## Client Protocol

The typed protocol lives in
`nanami/servers/sdk/rust/nanami-services/src/gfx/honoka.rs`. A client connects to
`honoka-service`, creates a window, attaches its framebuffer and event queue,
draws content, and submits damage or a present notification.

Reference clients include the shell, performance monitor, image viewer,
`honoka-client`, and `eg-test`.

## Build

From the repository root:

```bash
make image
make fs-image
```

Compositor changes must be verified in QEMU at the configured desktop
resolution. Check full-screen composition, cursor movement, window dragging,
transparency, repeated text updates, and close/reap behavior.
