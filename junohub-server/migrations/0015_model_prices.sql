ALTER TABLE user_models ADD COLUMN input_price_per_million REAL CHECK (input_price_per_million >= 0);
ALTER TABLE user_models ADD COLUMN output_price_per_million REAL CHECK (output_price_per_million >= 0);
