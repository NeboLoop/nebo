# Production Deployment

This folder contains everything needed for production deployment with zero-downtime updates.

## How It Works

1. **Push to main** → GitHub Actions builds Docker image → Pushes to GHCR
2. **DOSync** (running on your server) detects new image → Auto-deploys with zero downtime

## Setup

### 1. Create a DigitalOcean Droplet

```bash
# From project root
./scripts/deploy-digitalocean.sh your-app-name nyc1 s-2vcpu-4gb
```

Or manually create a droplet with Docker pre-installed.

### 2. SSH to Your Server

```bash
ssh root@your-server-ip
```

### 3. Create App Directory

```bash
mkdir -p /opt/app
cd /opt/app
```

### 4. Copy Deploy Files

Copy these files to `/opt/app/`:

- `compose.yaml`
- `dosync.yaml`
- `.env` (from `.env.example`, with production values)

### 5. Configure Environment

```bash
nano .env
```

Required variables:

- `ACCESS_SECRET` - JWT secret (generate: `openssl rand -hex 32`)
- `APP_DOMAIN` - Your domain (e.g., `myapp.com`)
- `ADMIN_EMAIL` - Email for Let's Encrypt notifications
- `STRIPE_SECRET_KEY` - Your Stripe secret key
- `STRIPE_PUBLISHABLE_KEY` - Your Stripe publishable key
- `STRIPE_WEBHOOK_SECRET` - Stripe webhook signing secret
- `GITHUB_REPOSITORY` - e.g., `your-org/your-app`
- `GITHUB_USERNAME` - Your GitHub username
- `GITHUB_PAT` - GitHub Personal Access Token (needs `read:packages` scope)
- `IMAGE_TAG` - Initial tag (DOSync will update this automatically)

### 6. Login to GitHub Container Registry

```bash
echo $GITHUB_PAT | docker login ghcr.io -u $GITHUB_USERNAME --password-stdin
```

### 7. Start Services

```bash
docker compose up -d
```

### 8. Point Your Domain

Add an A record pointing your domain to the server's IP address. Let's Encrypt certificates will be automatically provisioned.

## Files

| File           | Description                                       |
| -------------- | ------------------------------------------------- |
| `compose.yaml` | Production services: app, datasaver, dosync       |
| `dosync.yaml`  | DOSync configuration for auto-deployments         |

## Automated Backups (DataSaver)

DataSaver automatically backs up the SQLite database:

- **Schedule**: Daily at 2 AM UTC
- **Retention**: 7 daily, 4 weekly, 3 monthly (GFS rotation)
- **Storage**: Docker volume `db_backups`
- **Compression**: gzip

### Manual Backup

```bash
# Trigger immediate backup
docker compose exec datasaver datasaver backup

# List available backups
docker compose exec datasaver ls -la /backups
```

### Restore from Backup

```bash
# Stop the app first
docker compose stop app

# Restore (replace BACKUP_FILE with actual filename)
docker compose exec datasaver datasaver restore /backups/BACKUP_FILE.gz

# Restart app
docker compose start app
```

### Copy Backup to Host

```bash
docker cp datasaver:/backups/. ./local-backups/
```

## Monitoring

```bash
# View all logs
docker compose logs -f

# View app logs only
docker compose logs -f app

# View DOSync deployment logs
docker compose logs -f dosync

# Check service status
docker compose ps
```

## Manual Deployment

If you need to manually update (without waiting for DOSync):

```bash
# Pull latest image
docker compose pull app

# Recreate app container
docker compose up -d app
```

## Rollback

DOSync keeps backups. To rollback:

```bash
# Check available backups
ls backups/

# Restore a specific backup
# (Copy the compose.yaml from backup, then restart)
```

## Troubleshooting

### App won't start

```bash
docker compose logs app
```

### Database issues

```bash
# Check if volume exists
docker volume ls | grep app_data

# Inspect the volume
docker volume inspect deploy_app_data
```

### DOSync not detecting updates

```bash
docker compose logs dosync
# Check GITHUB_PAT is valid and has read:packages scope
```
