-- Historic prices cannot be reconstructed from the current route configuration.
ALTER TABLE user_request_logs ADD COLUMN input_price_per_million REAL;
ALTER TABLE user_request_logs ADD COLUMN output_price_per_million REAL;
