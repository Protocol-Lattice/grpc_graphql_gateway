# Changelog

All notable changes to this project will be documented in this file.

The format is based on Keep a Changelog, and this project adheres to Semantic Versioning.

## [0.9.2] - 2025-12-27

### Added
- Enhanced WAF with custom regex patterns support.
- Updated `WafConfig` with `custom_patterns` field.
- Added `check_custom` helper and integrated into validation functions.
- Updated documentation and examples.

## [0.9.1] - 2025-12-27

### Added
- OpenAPI security scheme parsing and RestConnector authentication support.
- `with_auth` builder method for OpenAPI parser to configure API key and bearer token credentials.
- Updated documentation and tests for security configuration.
