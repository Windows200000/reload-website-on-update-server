docker rm -f rust-watcher 2>/dev/null || true
docker build -f watcher-Dockerfile -t rust-watcher .
docker run -d \
  --name rust-watcher \
  -e RUST_LOG=info \
  -e EXTENSIONS=.html,.css,.js,.mjs,.json
  -p 8765:8765 \
  -v "[Insert your target folder here, I used: $(pwd)/public]:/site:ro" \
  rust-watcher
