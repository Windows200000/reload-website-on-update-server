# Reload-Website-on-Update Server

Docker container/Rust executable that automatically reloads a website if the files on the host are changed. What makes this unique is what might not make it the right project for you - it's dead simple and completely outside of the host serving content. It's essentially just a tiny little backend that tells you that some files changed.


## Usage

### 1. Clone the repo
```
git clone https://github.com/Windows200000/reload-website-on-update-server.git
cd ./reload-website-on-update-server
```

### 2. Configure for your setup
- Enter your server IP in `reload.js` and/or `reload_debug.js`.
- If you intend to run it as a container, configure the path to the folder you want to watch and the file types you want to watch in `build_watcher.sh` and/or `run_watcher.sh`.

### 3. Run the Backend
Either use the pre-built Docker image for your architecture...:
```
docker load -i rust-watcher-linux-amd64.tar
chmod +x run_watcher.sh
./run_watcher.sh
```
```
docker load -i rust-watcher-linux-arm64v8.tar
chmod +x run_watcher.sh
./run_watcher.sh
```
...build one using the docker file...:
```
chmod +x build_watcher.sh
./build_watcher.sh
```
...or run the executable for your architecture directly:
```
chmod +x rust-watcher-linux-amd64
WATCH_DIR="[Insert your target folder here, I used: $(pwd)/public]" \
HOST=0.0.0.0 \
PORT=8765 \
EXTENSIONS=.html,.css,.js \
POLL_INTERVAL=0.5 \
./rust-watcher-linux-amd64
```
```
chmod +x rust-watcher-linux-arm64v8
WATCH_DIR="[Insert your target folder here, I used: $(pwd)/public]" \
HOST=0.0.0.0 \
PORT=8765 \
EXTENSIONS=.html,.css,.js \
POLL_INTERVAL=0.5 \
./rust-watcher-linux-arm64v8
```
### 4. Open all firewalls for Port TCP/8765

### 5. Add the reload script to your website
Simply put ` <script src="reload.js"></script>` or ` <script src="reload_debug.js"></script>` in your HTML and move the `.js` files next to it.

## Additional notes
- By default, it will only serve HTTP, which will make most browsers reject it by default on HTTPS websites—the perfect tool for testing in production, since you'll be the only one getting it reloaded.

- I reviewed the code, and it should be safe to run. It doesn't use anything from requests except the IP they came from and doesn't send any dynamic content to clients either. Use at your own discretion.

- It probably won't work on Windows.

## AI Usage
This tool was made mostly by AI. Originally in Python, then I reviewed it and told the AI to stop sending a list of the whole OS paths that changed to the client for no reason and changed a bunch more stuff to not make it a security liability. The rewrite was made in a separate thread, and on a rough pass it did include all the changes, so it should be safe.
