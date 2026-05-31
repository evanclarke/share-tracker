-- ISO 10383 Market Identifier Code (MIC) registry: reference data populated from
-- the official ISO20022 ISO10383_MIC.csv by the `mic-import` maintenance job. It
-- is *not* the operational exchange table (it carries no currency / timezone /
-- settlement convention — the CSV has none of those); its sole role is to let the
-- exchange-MIC validation report flag curated `exchanges` whose MIC is unknown or
-- expired. No foreign keys: standalone reference data keyed by `mic`.
CREATE TABLE mic_registry (
    mic           TEXT PRIMARY KEY,           -- the MIC (ISO 10383), e.g. 'XASX'
    operating_mic TEXT NOT NULL,              -- parent operating MIC (== mic for operating entries)
    name          TEXT NOT NULL,              -- MARKET NAME-INSTITUTION DESCRIPTION
    country_code  TEXT NOT NULL,              -- ISO 3166 alpha-2 country code
    city          TEXT,                       -- city (nullable; some entries omit it)
    status        TEXT NOT NULL,              -- ISO STATUS: ACTIVE | UPDATED | EXPIRED
    expiry_date   TEXT                        -- ISO date 'YYYY-MM-DD' when EXPIRED, else NULL
);
