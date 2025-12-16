# Security Headers

The gateway automatically adds comprehensive security headers to all HTTP responses, providing defense-in-depth protection for production deployments.

## Headers Applied

### HTTP Strict Transport Security (HSTS)

```
Strict-Transport-Security: max-age=31536000; includeSubDomains
```

Forces browsers to only communicate over HTTPS for one year, including all subdomains. This prevents protocol downgrade attacks and cookie hijacking.

### Content Security Policy (CSP)

```
Content-Security-Policy: default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'
```

Restricts resource loading to same-origin, preventing XSS attacks by blocking inline scripts and external script sources.

### X-Content-Type-Options

```
X-Content-Type-Options: nosniff
```

Prevents browsers from MIME-sniffing responses, protecting against drive-by download attacks.

### X-Frame-Options

```
X-Frame-Options: DENY
```

Prevents the page from being embedded in iframes, protecting against clickjacking attacks.

### X-XSS-Protection

```
X-XSS-Protection: 1; mode=block
```

Enables browser's built-in XSS filtering (for legacy browsers).

### Referrer-Policy

```
Referrer-Policy: strict-origin-when-cross-origin
```

Controls referrer information sent with requests, limiting data leakage to third parties.

### Cache-Control

```
Cache-Control: no-store, no-cache, must-revalidate
```

Prevents caching of sensitive GraphQL responses by browsers and proxies.

## CORS Configuration

The gateway handles CORS preflight requests automatically:

### OPTIONS Requests

```
Access-Control-Allow-Origin: *
Access-Control-Allow-Methods: GET, POST, OPTIONS
Access-Control-Allow-Headers: Content-Type, Authorization, X-Request-ID
Access-Control-Max-Age: 86400
```

### Customizing CORS

For production deployments, you may want to restrict the `Access-Control-Allow-Origin` to specific domains. This can be configured in your gateway setup.

## Security Test Verification

The gateway includes a comprehensive security test suite (`test_security.sh`) that verifies all security headers:

```bash
./test_security.sh

# Expected output:
[PASS] T1: X-Content-Type-Options: nosniff
[PASS] T2: X-Frame-Options: DENY
[PASS] T12: HSTS Enabled
[PASS] T13: No X-Powered-By Header
[PASS] T14: Server Header Hidden
[PASS] T15: TRACE Rejected (405)
[PASS] T16: OPTIONS/CORS Supported (204)
```

## Best Practices

### For Production

1. **Always use HTTPS** - HSTS is automatically enabled
2. **Configure specific CORS origins** - Replace `*` with your domain
3. **Review CSP rules** - Adjust based on your frontend requirements
4. **Monitor security headers** - Use tools like securityheaders.com

### Additional Recommendations

- Enable TLS 1.3 on your reverse proxy (nginx/Cloudflare)
- Use certificate pinning for high-security applications
- Implement rate limiting at the edge
- Enable audit logging for security events
