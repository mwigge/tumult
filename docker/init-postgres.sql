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
('user-001', TRUE),
('user-002', TRUE),
('user-003', FALSE),
('user-004', TRUE),
('user-005', TRUE);

-- Connection tracking view (used by pool-utilization probe)
CREATE OR REPLACE VIEW connection_stats AS
SELECT
    COUNT(*) AS total_connections,
    COUNT(*) FILTER (WHERE state = 'active') AS active_connections,
    COUNT(*) FILTER (WHERE state = 'idle') AS idle_connections
FROM pg_stat_activity
WHERE datname = 'tumult_test';

-- Grant permissions
GRANT ALL PRIVILEGES ON ALL TABLES IN SCHEMA public TO tumult;
GRANT ALL PRIVILEGES ON ALL SEQUENCES IN SCHEMA public TO tumult;
