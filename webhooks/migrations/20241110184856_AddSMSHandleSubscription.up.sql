-- Add up migration script here
CREATE TABLE
  SmsHandleSubscriptions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid (),
    phone_number TEXT NOT NULL,
    handle TEXT NOT NULL,
    did TEXT NOT NULL,
    created_at TIMESTAMP
    WITH
      TIME ZONE NOT NULL DEFAULT CURRENT_TIMESTAMP,
      updated_at TIMESTAMP
    WITH
      TIME ZONE NOT NULL DEFAULT CURRENT_TIMESTAMP
  );
