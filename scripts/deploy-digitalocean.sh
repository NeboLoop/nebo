#!/bin/bash
# Deploy to DigitalOcean Droplet
# Prerequisites: doctl installed and authenticated (doctl auth init)
# Usage: ./deploy-digitalocean.sh [name] [region] [size]

set -e

DROPLET_NAME="${1:-saas-starter-prod}"
REGION="${2:-nyc1}"
SIZE="${3:-s-2vcpu-4gb}"

echo "======================================"
echo "  Deploying to DigitalOcean"
echo "======================================"
echo ""
echo "Droplet: $DROPLET_NAME"
echo "Region:  $REGION"
echo "Size:    $SIZE"
echo ""

# Check if doctl is installed
if ! command -v doctl &> /dev/null; then
    echo "Error: doctl is not installed"
    echo "Install it: https://docs.digitalocean.com/reference/doctl/how-to/install/"
    exit 1
fi

# Check if authenticated
if ! doctl account get &> /dev/null; then
    echo "Error: doctl is not authenticated"
    echo "Run: doctl auth init"
    exit 1
fi

# Get SSH key
SSH_KEY_ID=$(doctl compute ssh-key list --format ID --no-header | head -1)
if [ -z "$SSH_KEY_ID" ]; then
    echo "Error: No SSH keys found in your DigitalOcean account"
    echo "Add one at: https://cloud.digitalocean.com/account/security"
    exit 1
fi

echo "Creating droplet with Docker pre-installed..."
doctl compute droplet create "$DROPLET_NAME" \
    --region "$REGION" \
    --size "$SIZE" \
    --image docker-20-04 \
    --ssh-keys "$SSH_KEY_ID" \
    --wait

# Get IP address
IP=$(doctl compute droplet get "$DROPLET_NAME" --format PublicIPv4 --no-header)
echo ""
echo "Droplet created at: $IP"

# Wait for SSH to be ready
echo "Waiting for SSH to be ready..."
sleep 45

# Run setup on the droplet
echo "Setting up Docker Compose..."
ssh -o StrictHostKeyChecking=no -o ConnectTimeout=30 root@"$IP" << 'ENDSSH'
mkdir -p /opt/app
cd /opt/app

# Create .env template
cat > .env << 'EOF'
# Required
PRODUCTION_MODE=true
APP_DOMAIN=your-domain.com
ADMIN_EMAIL=admin@your-domain.com
ACCESS_SECRET=your-32-char-secret-here-change-me
SQLITE_PATH=/app/data/nebo.db

# Admin
ADMIN_USERNAME=admin@your-domain.com
ADMIN_PASSWORD=change-this-password
EOF

echo "Server setup complete!"
ENDSSH

echo ""
echo "======================================"
echo "  Deployment Complete!"
echo "======================================"
echo ""
echo "Your droplet is ready at: $IP"
echo ""
echo "Next steps:"
echo "1. SSH to your server: ssh root@$IP"
echo "2. Clone your repo: git clone https://github.com/your-org/your-app.git /opt/app"
echo "3. Configure .env: nano /opt/app/.env"
echo "4. Start: cd /opt/app && docker compose -f compose.prod.yaml up -d"
echo ""
echo "To set up a domain:"
echo "1. Point your domain's A record to $IP"
echo "2. Update APP_DOMAIN in .env"
echo "3. The app uses built-in Let's Encrypt autocert for HTTPS"
echo ""
