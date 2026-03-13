# Komodo Mail Bridge

A small Flask service that receives Komodo webhook alerts and forwards them as email via SMTP.

## Files

```text
komodo-mail-bridge/
├─ Dockerfile
├─ app.py
└─ README.md
```

## Published image

GitHub Actions publishes the container to GHCR as `ghcr.io/<owner>/komodo-mail-bridge`.
The `latest` tag is now explicitly published from the default branch, so this works:

```bash
docker pull ghcr.io/<owner>/komodo-mail-bridge:latest
```

## Build

```bash
docker build -t komodo-mail-bridge:latest .
```

## Run

```bash
docker run --rm -p 8000:8000 --env-file .env komodo-mail-bridge:latest
```

## Environment variables

Set these values (for example in a `.env` file):

```env
SMTP_HOST=postfix
SMTP_PORT="25"
FROM_ADDR=komodo@example.com
TO_ADDR=mail@example.com
SUBJECT_PREFIX="[Komodo]"
```

## Endpoints

- `GET /health` returns `{ "ok": true }`
- `POST /komodo` accepts Komodo alert JSON and sends an email

## Example webhook request

```bash
curl -X POST http://localhost:8000/komodo \
  -H "Content-Type: application/json" \
  -d '{
    "level": "OK",
    "resolved": true,
    "ts": 1710000000000,
    "resolved_ts": 1710000300000,
    "target": {"type": "Server", "id": "srv-01"},
    "data": {
      "type": "Test",
      "data": {"name": "alerter", "id": "alert-123"}
    }
  }'
```

## Notes

- The service sends multipart emails (plain text + HTML).
- If `TO_ADDR` is empty, `/komodo` returns HTTP 400.
- SMTP send failures return HTTP 500 with the SMTP error.
