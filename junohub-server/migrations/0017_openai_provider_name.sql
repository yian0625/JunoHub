UPDATE user_providers AS old
SET name = 'OpenAI'
WHERE old.provider_type = 'sub2api'
  AND old.name = 'OpenAI Codex'
  AND NOT EXISTS (
    SELECT 1 FROM user_providers AS other
    WHERE other.user_id = old.user_id AND other.id <> old.id
      AND lower(other.name) = 'openai'
  );
