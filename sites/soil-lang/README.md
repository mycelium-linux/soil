# soil-lang.org

The first landing page for Soil, written as a single `.soil` view.

Preview it full-screen through the dev proxy:

```sh
# 1. Rust host (no rebuild needed for the site source)
target/debug/soil-ui-proto --source sites/soil-lang/soil-lang.soil --port 7882

# 2. Serve the current web assets from disk and proxy the API
SOIL_UI_UPSTREAM=http://127.0.0.1:7882 SOIL_UI_PORT=7883 \
  node crates/soil-ui-proto/web/dev-proxy.mjs

# 3. Open the bare preview (no editor panel)
open 'http://127.0.0.1:7883/?bare=1'
```

The page is a static landscape: nav, hero, why, substrate, learn, spores,
community, footer. It has no actions and no state.

Build the static page and every example through the Soil standard library:

```sh
./sites/soil-lang/build.sh
# sites/soil-lang/dist/index.html
# sites/soil-lang/dist/examples/*.html
```

Cargo is frozen for this step: the script uses an existing `target/debug/soil-lang`
and fails rather than building. The output is plain HTML with server-rendered
syntax highlighting. `dist/` is generated and checked in for deployment; the
live component path is the dev proxy at http://127.0.0.1:7883/?bare=1.
