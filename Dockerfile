# Multi-stage Dockerfile for Gobot
# Single all-in-one binary with embedded SvelteKit frontend
# Usage: docker build -t nebo .

# Development stage with Air for hot reloading
FROM golang:1.25-alpine AS development

WORKDIR /app

# Install dependencies
RUN apk add --no-cache git build-base nodejs npm

# Install pnpm globally
RUN npm install -g pnpm

# Install Air for hot reloading
RUN go install github.com/air-verse/air@v1.61.5

# Copy go mod files
COPY go.mod go.sum ./
RUN go mod download

# Copy source code
COPY . .

# Build frontend for embedding
RUN cd app && CI=true pnpm install && pnpm run build

# Expose port (Go backend runs on 8888)
EXPOSE 8888

# Use Air for hot reloading in development
CMD ["air"]

# Frontend builder stage
FROM node:20-alpine AS frontend-builder

WORKDIR /app

# Install pnpm
RUN npm install -g pnpm

# Copy frontend package files
COPY app/package.json app/pnpm-lock.yaml ./
RUN pnpm install

# Copy frontend source
COPY app/src ./src
COPY app/static ./static
COPY app/scripts ./scripts
COPY app/svelte.config.js app/vite.config.ts app/tsconfig.json ./

# Copy nebo.yaml for pricing (read at build time by +page.server.ts)
COPY etc/nebo.yaml ../etc/nebo.yaml

# Generate SvelteKit files and build frontend for production
RUN pnpm exec svelte-kit sync && pnpm run build

# Production builder stage
FROM golang:1.25-alpine AS builder

WORKDIR /app

# Install build dependencies
RUN apk add --no-cache git build-base

# Copy go mod files
COPY go.mod go.sum ./
RUN go mod download

# Copy Go source code and all packages
COPY *.go ./
COPY agent/ ./agent/
COPY cmd/ ./cmd/
COPY internal/ ./internal/
COPY app/ ./app/
COPY etc/ ./etc/

# Copy built frontend from frontend-builder
COPY --from=frontend-builder /app/build ./app/build

# Build the all-in-one binary
RUN CGO_ENABLED=0 GOOS=linux go build -a -installsuffix cgo \
    -ldflags="-w -s" \
    -o /app/bin/nebo .

# Final production stage
FROM alpine:latest AS production

RUN apk --no-cache add ca-certificates curl wget tzdata

WORKDIR /app

# Copy the all-in-one binary
COPY --from=builder /app/bin/nebo ./nebo

# Copy configuration files
COPY etc/ ./etc/

# Copy database migrations (embedded in binary via goose, but kept for reference)
COPY internal/db/migrations/ ./internal/db/migrations/

# Create necessary directories
RUN mkdir -p /app/certs /app/backups

# Expose ports (80 for HTTP redirect, 443 for HTTPS, 8888 for backend)
EXPOSE 80 443 8888

# Health check on internal backend port
HEALTHCHECK --interval=30s --timeout=10s --retries=3 --start-period=40s \
  CMD wget -q -O /dev/null http://localhost:8888/health || exit 1

# Run the server
CMD ["./nebo"]
