ALTER TABLE nodes
    ADD COLUMN protocols TEXT[] NOT NULL DEFAULT ARRAY['tcp', 'udp', 'http']::text[];
