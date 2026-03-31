-- Tumult e2e test database initialization
-- Applied automatically when the postgres container starts for the first time.

-- Test table for chaos experiments
CREATE TABLE IF NOT EXISTS app_sessions (
    id SERIAL PRIMARY KEY,
    user_id VARCHAR(64) NOT NULL,
    created_at TIMESTAMP DEFAULT NOW(),
    active BOOLEAN DEFAULT TRUE
);

-- Insert sample data for probe testing
INSERT INTO app_sessions (user_id, active) VALUES
    ('user-001', true),
    ('user-002', true),
    ('user-003', false),
    ('user-004', true),
    ('user-005', true);

-- Connection tracking view (used by pool-utilization probe)
CREATE OR REPLACE VIEW connection_stats AS
SELECT
    count(*) AS total_connections,
    count(*) FILTER (WHERE state = 'active') AS active_connections,
    count(*) FILTER (WHERE state = 'idle') AS idle_connections
FROM pg_stat_activity
WHERE datname = 'tumult_test';

-- Grant permissions
GRANT ALL PRIVILEGES ON ALL TABLES IN SCHEMA public TO tumult;
GRANT ALL PRIVILEGES ON ALL SEQUENCES IN SCHEMA public TO tumult;
