---
name: certificates
description: TLS or SSL certificates. Checking when they expire, renewing them with certbot or acme.sh, and reloading the web server after.
---
# Certificates

## Check
- Use a tls step for each site. It gives days_left, expires and trusted. Warn at a limit, like 21 days, and again close to the end.
- For certificate files on a server: `openssl x509 -enddate -noout -in <file>`.
- When the user says "all my sites", find them each run from the web server's config or the user's list, not from a list made today.

## Renew
- Use the tool that made it: `certbot renew` (it only renews what is due, and `--dry-run` previews), or `acme.sh --cron`.
- After a renewal, test the web server's config (`nginx -t`, `apachectl configtest`), then reload it, not restart.
- Never delete or replace private keys by hand.

## Report
- Each site, its days left, and what was renewed.
