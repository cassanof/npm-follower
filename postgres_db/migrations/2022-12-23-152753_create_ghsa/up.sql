CREATE TABLE ghsa (
  id TEXT PRIMARY KEY NOT NULL,
  -- GHSA id
  severity TEXT NOT NULL,
  -- Severity of the vulnerability
  description TEXT NOT NULL,
  -- Description of the vulnerability
  summary TEXT NOT NULL,
  -- Summary of the vulnerability
  withdrawn_at TIMESTAMP WITH TIME ZONE,
  -- Date when the vulnerability was withdrawn (if any)
  published_at TIMESTAMP WITH TIME ZONE NOT NULL,
  -- Date when the vulnerability was published
  updated_at TIMESTAMP WITH TIME ZONE NOT NULL,
  -- Date when the vulnerability was last updated
  refs TEXT [] NOT NULL,
  -- References (URLS) to the vulnerability
  cvss_score real,
  -- CVSS score of the vulnerability
  cvss_vector TEXT -- CVSS vector of the vulnerability
);


CREATE TABLE vulnerabilities (
  id BIGINT PRIMARY KEY GENERATED ALWAYS AS IDENTITY,
  ghsa_id TEXT NOT NULL REFERENCES ghsa (id),
  package_name TEXT NOT NULL,
  vulnerable_version_lower_bound semver,
  vulnerable_version_lower_bound_inclusive BOOLEAN NOT NULL,
  vulnerable_version_upper_bound semver,
  vulnerable_version_upper_bound_inclusive BOOLEAN NOT NULL,
  first_patched_version semver,
  CONSTRAINT unique_range UNIQUE (
    ghsa_id,
    package_name,
    vulnerable_version_lower_bound,
    vulnerable_version_lower_bound_inclusive,
    vulnerable_version_upper_bound,
    vulnerable_version_upper_bound_inclusive
  )
);

CREATE TABLE cwes (
  id TEXT PRIMARY KEY NOT NULL,
  name TEXT NOT NULL,
  description TEXT NOT NULL
);

CREATE TABLE ghsa_cwe_relation (
  ghsa_id TEXT NOT NULL,
  cwe_id TEXT NOT NULL,
  PRIMARY KEY (ghsa_id, cwe_id)
);
