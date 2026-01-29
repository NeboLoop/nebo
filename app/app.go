package app

import (
	"embed"
	"fmt"
	"io"
	"io/fs"
	"net/http"
	"os"
	"strings"
	"time"
)

// Run `go generate ./...` before building the Go binary to refresh the embedded SPA build.
//go:generate sh -c "command -v pnpm >/dev/null 2>&1 || { echo 'pnpm not found. Please install pnpm first.' >&2; exit 1; }"
//go:generate pnpm run build
//go:generate sh -c "[ -d build ] || { echo 'Build failed - no build directory found' >&2; exit 1; }"

// Embed the entire build folder
//
//go:embed all:build
var buildFS embed.FS

var ServerHost string

// DevMode controls whether to use local filesystem instead of embedded FS
var DevMode bool

// QuietMode suppresses verbose output during startup
var QuietMode bool

// SetServerHost sets the server host for server.json
func SetServerHost(host string, port int, useHTTPS bool) {
	protocol := "http"
	if useHTTPS {
		protocol = "https"
	}

	if (useHTTPS && port == 443) || (!useHTTPS && port == 80) {
		ServerHost = fmt.Sprintf("%s://%s", protocol, host)
	} else {
		ServerHost = fmt.Sprintf("%s://%s:%d", protocol, host, port)
	}

	fmt.Printf("Server Host set to: %s\n", ServerHost)
}

// NotFoundHandler serves index.html for SPA routing
func NotFoundHandler(spaFS fs.FS) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if strings.HasPrefix(r.URL.Path, "/api") {
			http.Error(w, "Not Found", http.StatusNotFound)
			return
		}

		path := r.URL.Path
		if strings.Contains(path, ".") && !strings.HasSuffix(path, "/") {
			http.Error(w, "Not Found", http.StatusNotFound)
			return
		}

		file, err := spaFS.Open("index.html")
		if err != nil {
			http.Error(w, "Not Found", http.StatusNotFound)
			return
		}
		defer file.Close()

		w.Header().Set("Content-Type", "text/html")
		w.WriteHeader(http.StatusOK)
		io.Copy(w, file)
	})
}

// SPAHandler returns a handler that serves static files with SPA fallback
func SPAHandler(spaFS fs.FS) http.Handler {
	fileServer := http.FileServer(http.FS(spaFS))

	serveFile := func(w http.ResponseWriter, r *http.Request, path string) {
		file, err := spaFS.Open(path)
		if err != nil {
			http.NotFound(w, r)
			return
		}
		defer file.Close()

		stat, err := file.Stat()
		if err != nil {
			http.NotFound(w, r)
			return
		}

		http.ServeContent(w, r, stat.Name(), stat.ModTime(), file.(io.ReadSeeker))
	}

	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		path := strings.TrimPrefix(r.URL.Path, "/")
		if path == "" {
			path = "index.html"
		}

		if path != "" && strings.HasSuffix(path, "/") {
			http.Redirect(w, r, "/"+strings.TrimSuffix(path, "/"), http.StatusMovedPermanently)
			return
		}

		file, err := spaFS.Open(path)
		if err == nil {
			stat, err := file.Stat()
			file.Close()

			if err == nil && !stat.IsDir() {
				fileServer.ServeHTTP(w, r)
				return
			}
		}

		if !strings.Contains(path, ".") {
			htmlPath := path + ".html"
			if _, err := spaFS.Open(htmlPath); err == nil {
				serveFile(w, r, htmlPath)
				return
			}
		}

		if strings.Contains(path, ".") {
			http.NotFound(w, r)
			return
		}

		if _, err := spaFS.Open("200.html"); err == nil {
			serveFile(w, r, "200.html")
			return
		}

		serveFile(w, r, "index.html")
	})
}

// FileSystem returns the SPA filesystem.
func FileSystem() (fs.FS, error) {
	if DevMode {
		localPath := "app/build"
		if _, err := os.Stat(localPath); err != nil {
			return nil, fmt.Errorf("dev mode: build directory not found at %s - run 'cd app && pnpm build' first", localPath)
		}
		fmt.Println("Using local filesystem for SPA (dev mode): app/build")
		return &serverJSONFS{os.DirFS(localPath)}, nil
	}

	sub, err := fs.Sub(buildFS, "build")
	if err != nil {
		return nil, err
	}
	return &serverJSONFS{sub}, nil
}

type serverJSONFS struct {
	fs.FS
}

func (s *serverJSONFS) Open(name string) (fs.File, error) {
	if name == "server.json" || name == "/server.json" {
		content := `{"server": "` + ServerHost + `"}`
		return &serverJSONFile{content: content}, nil
	}
	return s.FS.Open(name)
}

type serverJSONFile struct {
	content string
	pos     int
}

func (f *serverJSONFile) Read(p []byte) (int, error) {
	if f.pos >= len(f.content) {
		return 0, io.EOF
	}
	n := copy(p, f.content[f.pos:])
	f.pos += n
	return n, nil
}

func (f *serverJSONFile) Close() error { return nil }

func (f *serverJSONFile) Stat() (fs.FileInfo, error) {
	return &serverJSONInfo{size: int64(len(f.content))}, nil
}

type serverJSONInfo struct {
	size int64
}

func (i *serverJSONInfo) Name() string       { return "server.json" }
func (i *serverJSONInfo) Size() int64        { return i.size }
func (i *serverJSONInfo) Mode() fs.FileMode  { return 0444 }
func (i *serverJSONInfo) ModTime() time.Time { return time.Now() }
func (i *serverJSONInfo) IsDir() bool        { return false }
func (i *serverJSONInfo) Sys() interface{}   { return nil }
