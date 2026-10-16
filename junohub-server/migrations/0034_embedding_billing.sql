-- Snapshot the public contract so asynchronous accounting survives route edits.
ALTER TABLE gateway_tasks ADD COLUMN api_type TEXT NOT NULL DEFAULT 'auto';
UPDATE gateway_tasks SET api_type=COALESCE(
    (SELECT json_extract(m.api_config, '$.api_type') FROM user_models m WHERE m.id=gateway_tasks.model_id),
    'auto'
);

-- Embeddings have input usage; returned vector dimensions are not output tokens.
UPDATE user_models SET output_price_per_million=NULL
WHERE json_extract(api_config, '$.api_type')='embeddings';
UPDATE gateway_tasks SET output_price=NULL WHERE api_type='embeddings';
