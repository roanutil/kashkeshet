-- Messages by the time they were sent, so the web page's dashboard can
-- count each channel's recent messages without reading every message.
CREATE INDEX messages_by_time ON messages(created_at);
