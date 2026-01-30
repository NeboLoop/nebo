package httputil

import (
	"encoding/json"
	"net/http"
	"reflect"
	"strconv"
	"strings"

	"github.com/go-chi/chi/v5"
)

// Parse parses the request into the given struct.
// Supports:
// - JSON body (for POST/PUT/PATCH)
// - Path parameters via `path:"name"` struct tag (using chi.URLParam)
// - Query parameters via `form:"name"` struct tag
// Supports JSON body, path parameters, and query parameters.
func Parse(r *http.Request, v any) error {
	val := reflect.ValueOf(v)
	if val.Kind() != reflect.Ptr || val.IsNil() {
		return nil
	}
	val = val.Elem()
	if val.Kind() != reflect.Struct {
		return nil
	}

	typ := val.Type()

	// Parse struct fields
	for i := 0; i < val.NumField(); i++ {
		field := val.Field(i)
		if !field.CanSet() {
			continue
		}

		structField := typ.Field(i)

		// Check for path tag
		if pathTag := structField.Tag.Get("path"); pathTag != "" {
			pathVal := chi.URLParam(r, pathTag)
			if pathVal != "" {
				setFieldValue(field, pathVal)
			}
		}

		// Check for form/query tag
		if formTag := structField.Tag.Get("form"); formTag != "" {
			queryVal := r.URL.Query().Get(formTag)
			if queryVal != "" {
				setFieldValue(field, queryVal)
			}
		}
	}

	// Parse JSON body if present (for POST/PUT/PATCH with content)
	if r.Body != nil && r.ContentLength > 0 {
		contentType := r.Header.Get("Content-Type")
		if strings.HasPrefix(contentType, "application/json") || contentType == "" {
			if err := json.NewDecoder(r.Body).Decode(v); err != nil {
				return err
			}
		}
	}

	return nil
}

// setFieldValue sets a struct field value from a string
func setFieldValue(field reflect.Value, value string) {
	switch field.Kind() {
	case reflect.String:
		field.SetString(value)
	case reflect.Int, reflect.Int8, reflect.Int16, reflect.Int32, reflect.Int64:
		if i, err := strconv.ParseInt(value, 10, 64); err == nil {
			field.SetInt(i)
		}
	case reflect.Uint, reflect.Uint8, reflect.Uint16, reflect.Uint32, reflect.Uint64:
		if i, err := strconv.ParseUint(value, 10, 64); err == nil {
			field.SetUint(i)
		}
	case reflect.Bool:
		if b, err := strconv.ParseBool(value); err == nil {
			field.SetBool(b)
		}
	case reflect.Float32, reflect.Float64:
		if f, err := strconv.ParseFloat(value, 64); err == nil {
			field.SetFloat(f)
		}
	}
}

// PathVar returns a path variable from the request (chi.URLParam wrapper)
func PathVar(r *http.Request, name string) string {
	return chi.URLParam(r, name)
}

// QueryInt returns a query parameter as int with a default value
func QueryInt(r *http.Request, name string, defaultVal int) int {
	val := r.URL.Query().Get(name)
	if val == "" {
		return defaultVal
	}
	if i, err := strconv.Atoi(val); err == nil {
		return i
	}
	return defaultVal
}

// QueryString returns a query parameter as string with a default value
func QueryString(r *http.Request, name string, defaultVal string) string {
	val := r.URL.Query().Get(name)
	if val == "" {
		return defaultVal
	}
	return val
}

// OkJSON writes a JSON response with 200 OK status
func OkJSON(w http.ResponseWriter, v any) {
	w.Header().Set("Content-Type", "application/json; charset=utf-8")
	w.WriteHeader(http.StatusOK)
	json.NewEncoder(w).Encode(v)
}

// WriteJSON writes a JSON response with the given status code
func WriteJSON(w http.ResponseWriter, status int, v any) {
	w.Header().Set("Content-Type", "application/json; charset=utf-8")
	w.WriteHeader(status)
	json.NewEncoder(w).Encode(v)
}

// ErrorResponse is the standard error response format
type ErrorResponse struct {
	Code    int    `json:"code"`
	Message string `json:"message"`
}

// Error writes an error response with the appropriate status code
func Error(w http.ResponseWriter, err error) {
	ErrorWithCode(w, http.StatusBadRequest, err.Error())
}

// ErrorWithCode writes an error response with a specific status code
func ErrorWithCode(w http.ResponseWriter, code int, message string) {
	w.Header().Set("Content-Type", "application/json; charset=utf-8")
	w.WriteHeader(code)
	json.NewEncoder(w).Encode(ErrorResponse{
		Code:    code,
		Message: message,
	})
}

// Unauthorized writes a 401 unauthorized response
func Unauthorized(w http.ResponseWriter, message string) {
	if message == "" {
		message = "unauthorized"
	}
	ErrorWithCode(w, http.StatusUnauthorized, message)
}

// NotFound writes a 404 not found response
func NotFound(w http.ResponseWriter, message string) {
	if message == "" {
		message = "not found"
	}
	ErrorWithCode(w, http.StatusNotFound, message)
}

// InternalError writes a 500 internal server error response
func InternalError(w http.ResponseWriter, message string) {
	if message == "" {
		message = "internal server error"
	}
	ErrorWithCode(w, http.StatusInternalServerError, message)
}
