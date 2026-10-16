-- Replace only known generated names; preserve custom names and avoid collisions.
UPDATE user_providers AS old
SET name = substr(old.name, 11)
WHERE old.provider_type = 'sub2api'
  AND old.name IN ('Sub2API · OpenAI Codex', 'Sub2API · OpenAI', 'Sub2API · Claude', 'Sub2API · Gemini', 'Sub2API · Antigravity', 'Sub2API · Grok')
  AND NOT EXISTS (
    SELECT 1 FROM user_providers AS other
    WHERE other.user_id = old.user_id AND other.id <> old.id
      AND lower(other.name) = lower(substr(old.name, 11))
  );
