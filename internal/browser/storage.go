package browser

import (
	"context"
	"fmt"
	"time"

	"github.com/playwright-community/playwright-go"
)

// Cookie represents a browser cookie.
type Cookie struct {
	Name     string  `json:"name"`
	Value    string  `json:"value"`
	URL      string  `json:"url,omitempty"`
	Domain   string  `json:"domain,omitempty"`
	Path     string  `json:"path,omitempty"`
	Expires  float64 `json:"expires,omitempty"`
	HTTPOnly bool    `json:"httpOnly,omitempty"`
	Secure   bool    `json:"secure,omitempty"`
	SameSite string  `json:"sameSite,omitempty"` // "Strict", "Lax", "None"
}

// StorageKind is the type of web storage.
type StorageKind string

const (
	StorageLocal   StorageKind = "local"
	StorageSession StorageKind = "session"
)

// GetCookies returns all cookies for the current context.
func (p *Page) GetCookies(ctx context.Context) ([]Cookie, error) {
	if p.closed {
		return nil, fmt.Errorf("page is closed")
	}

	pwCookies, err := p.page.Context().Cookies()
	if err != nil {
		return nil, fmt.Errorf("get cookies failed: %w", err)
	}

	cookies := make([]Cookie, len(pwCookies))
	for i, c := range pwCookies {
		sameSite := ""
		if c.SameSite != nil {
			sameSite = string(*c.SameSite)
		}
		cookies[i] = Cookie{
			Name:     c.Name,
			Value:    c.Value,
			Domain:   c.Domain,
			Path:     c.Path,
			Expires:  c.Expires,
			HTTPOnly: c.HttpOnly,
			Secure:   c.Secure,
			SameSite: sameSite,
		}
	}

	return cookies, nil
}

// SetCookie sets a cookie.
func (p *Page) SetCookie(ctx context.Context, cookie Cookie) error {
	if p.closed {
		return fmt.Errorf("page is closed")
	}

	if cookie.Name == "" {
		return fmt.Errorf("cookie name is required")
	}

	// Must have either URL or domain+path
	hasURL := cookie.URL != ""
	hasDomainPath := cookie.Domain != "" && cookie.Path != ""
	if !hasURL && !hasDomainPath {
		return fmt.Errorf("cookie requires url, or domain+path")
	}

	var sameSite *playwright.SameSiteAttribute
	switch cookie.SameSite {
	case "Strict":
		sameSite = playwright.SameSiteAttributeStrict
	case "None":
		sameSite = playwright.SameSiteAttributeNone
	case "Lax", "":
		sameSite = playwright.SameSiteAttributeLax
	}

	pwCookie := playwright.OptionalCookie{
		Name:     cookie.Name,
		Value:    cookie.Value,
		SameSite: sameSite,
	}

	if cookie.Domain != "" {
		pwCookie.Domain = playwright.String(cookie.Domain)
	}
	if cookie.Path != "" {
		pwCookie.Path = playwright.String(cookie.Path)
	}
	if cookie.URL != "" {
		pwCookie.URL = playwright.String(cookie.URL)
	}
	if cookie.Expires > 0 {
		pwCookie.Expires = playwright.Float(cookie.Expires)
	}
	if cookie.HTTPOnly {
		pwCookie.HttpOnly = playwright.Bool(cookie.HTTPOnly)
	}
	if cookie.Secure {
		pwCookie.Secure = playwright.Bool(cookie.Secure)
	}

	err := p.page.Context().AddCookies([]playwright.OptionalCookie{pwCookie})
	if err != nil {
		return fmt.Errorf("set cookie failed: %w", err)
	}

	return nil
}

// ClearCookies clears all cookies.
func (p *Page) ClearCookies(ctx context.Context) error {
	if p.closed {
		return fmt.Errorf("page is closed")
	}

	err := p.page.Context().ClearCookies()
	if err != nil {
		return fmt.Errorf("clear cookies failed: %w", err)
	}

	return nil
}

// GetStorage gets values from localStorage or sessionStorage.
func (p *Page) GetStorage(ctx context.Context, kind StorageKind, key string) (map[string]string, error) {
	if p.closed {
		return nil, fmt.Errorf("page is closed")
	}

	storeName := "localStorage"
	if kind == StorageSession {
		storeName = "sessionStorage"
	}

	var script string
	if key != "" {
		script = fmt.Sprintf(`
			(() => {
				const value = window.%s.getItem(%q);
				return value === null ? {} : { %q: value };
			})()
		`, storeName, key, key)
	} else {
		script = fmt.Sprintf(`
			(() => {
				const store = window.%s;
				const result = {};
				for (let i = 0; i < store.length; i++) {
					const k = store.key(i);
					if (k) {
						const v = store.getItem(k);
						if (v !== null) {
							result[k] = v;
						}
					}
				}
				return result;
			})()
		`, storeName)
	}

	result, err := p.page.Evaluate(script)
	if err != nil {
		return nil, fmt.Errorf("get storage failed: %w", err)
	}

	// Convert result to map[string]string
	values := make(map[string]string)
	if m, ok := result.(map[string]interface{}); ok {
		for k, v := range m {
			if s, ok := v.(string); ok {
				values[k] = s
			}
		}
	}

	return values, nil
}

// SetStorage sets a value in localStorage or sessionStorage.
func (p *Page) SetStorage(ctx context.Context, kind StorageKind, key, value string) error {
	if p.closed {
		return fmt.Errorf("page is closed")
	}

	if key == "" {
		return fmt.Errorf("key is required")
	}

	storeName := "localStorage"
	if kind == StorageSession {
		storeName = "sessionStorage"
	}

	script := fmt.Sprintf(`window.%s.setItem(%q, %q)`, storeName, key, value)
	_, err := p.page.Evaluate(script)
	if err != nil {
		return fmt.Errorf("set storage failed: %w", err)
	}

	return nil
}

// ClearStorage clears localStorage or sessionStorage.
func (p *Page) ClearStorage(ctx context.Context, kind StorageKind) error {
	if p.closed {
		return fmt.Errorf("page is closed")
	}

	storeName := "localStorage"
	if kind == StorageSession {
		storeName = "sessionStorage"
	}

	script := fmt.Sprintf(`window.%s.clear()`, storeName)
	_, err := p.page.Evaluate(script)
	if err != nil {
		return fmt.Errorf("clear storage failed: %w", err)
	}

	return nil
}

// RemoveStorage removes a key from localStorage or sessionStorage.
func (p *Page) RemoveStorage(ctx context.Context, kind StorageKind, key string) error {
	if p.closed {
		return fmt.Errorf("page is closed")
	}

	if key == "" {
		return fmt.Errorf("key is required")
	}

	storeName := "localStorage"
	if kind == StorageSession {
		storeName = "sessionStorage"
	}

	script := fmt.Sprintf(`window.%s.removeItem(%q)`, storeName, key)
	_, err := p.page.Evaluate(script)
	if err != nil {
		return fmt.Errorf("remove storage failed: %w", err)
	}

	return nil
}

// SaveStorageState saves cookies and storage to a file (for session persistence).
func (p *Page) SaveStorageState(ctx context.Context, path string) error {
	if p.closed {
		return fmt.Errorf("page is closed")
	}

	_, err := p.page.Context().StorageState(path)
	if err != nil {
		return fmt.Errorf("save storage state failed: %w", err)
	}

	return nil
}

// SetCookieExpiry is a helper to create a cookie expiry timestamp.
func SetCookieExpiry(d time.Duration) float64 {
	return float64(time.Now().Add(d).Unix())
}
