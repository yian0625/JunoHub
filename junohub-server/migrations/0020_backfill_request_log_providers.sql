UPDATE user_request_logs
SET provider_id = COALESCE((
  SELECT m.provider_id
  FROM user_models m
  WHERE m.user_id = user_request_logs.user_id
    AND m.alias = user_request_logs.model_alias
  LIMIT 1
), 0)
WHERE provider_id = 0;

UPDATE user_request_logs
SET provider_name = COALESCE((
  SELECT p.name
  FROM user_providers p
  WHERE p.id = user_request_logs.provider_id
  LIMIT 1
), provider_type)
WHERE provider_name = '';
