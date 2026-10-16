ALTER TABLE gateway_tasks ADD COLUMN poll_after INTEGER NOT NULL DEFAULT 0;
ALTER TABLE gateway_tasks ADD COLUMN poll_lease_until INTEGER NOT NULL DEFAULT 0;
ALTER TABLE gateway_tasks ADD COLUMN poll_owner TEXT NOT NULL DEFAULT '';
ALTER TABLE gateway_tasks ADD COLUMN poll_failures INTEGER NOT NULL DEFAULT 0;
CREATE INDEX gateway_tasks_poll_due ON gateway_tasks(result_json,poll_after,poll_lease_until);
