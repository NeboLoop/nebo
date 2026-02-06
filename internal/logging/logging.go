package logging

import (
	"context"
	"log"
	"os"
)

var (
	disabled = false
	logger   = log.New(os.Stdout, "", log.LstdFlags)
)

// Disable turns off all logging
func Disable() {
	disabled = true
}

// Enable turns logging back on
func Enable() {
	disabled = false
}

// Info logs an info message
func Info(v ...any) {
	if !disabled {
		logger.Println(v...)
	}
}

// Infof logs a formatted info message
func Infof(format string, v ...any) {
	if !disabled {
		logger.Printf(format, v...)
	}
}

// Error logs an error message
func Error(v ...any) {
	if !disabled {
		logger.Println(v...)
	}
}

// Errorf logs a formatted error message
func Errorf(format string, v ...any) {
	if !disabled {
		logger.Printf(format, v...)
	}
}

// Warn logs a warning message
func Warn(v ...any) {
	if !disabled {
		logger.Println(v...)
	}
}

// Warnf logs a formatted warning message
func Warnf(format string, v ...any) {
	if !disabled {
		logger.Printf(format, v...)
	}
}

// Debug logs a debug message (same as Info when not disabled)
func Debug(v ...any) {
	if !disabled {
		logger.Println(v...)
	}
}

// Debugf logs a formatted debug message
func Debugf(format string, v ...any) {
	if !disabled {
		logger.Printf(format, v...)
	}
}

// Logger is a simple logger that can be embedded in structs
type Logger struct{}

// WithContext creates a new Logger (context is ignored, for API compatibility)
func WithContext(ctx context.Context) Logger {
	return Logger{}
}

// Info logs an info message
func (l Logger) Info(v ...any) {
	Info(v...)
}

// Infof logs a formatted info message
func (l Logger) Infof(format string, v ...any) {
	Infof(format, v...)
}

// Error logs an error message
func (l Logger) Error(v ...any) {
	Error(v...)
}

// Errorf logs a formatted error message
func (l Logger) Errorf(format string, v ...any) {
	Errorf(format, v...)
}
