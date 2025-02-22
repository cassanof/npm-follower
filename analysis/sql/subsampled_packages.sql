CREATE TABLE solving_analysis.subsampled_packages AS 

WITH filtered_updates as (
  SELECT *
  from metadata_analysis.all_updates
  where ty <> 'zero_to_something' and 
  (from_semver).prerelease is null and
  (to_semver).prerelease is null and
  (from_semver).build is null and
  (to_semver).build is null and
  package_id IN (select pkg from metadata_analysis.possible_install_deps)
),
ranked_updates as (
  SELECT *,
    ROW_NUMBER() over (
      partition by package_id
      order by to_created desc
    ) as date_rank
  FROM filtered_updates
)
SELECT package_id,
  from_id,
  to_id,
  from_semver,
  to_semver,
  from_created,
  to_created,
  ty,
  FALSE as patches_vuln,
  FALSE as introduced_vuln
FROM ranked_updates
WHERE date_rank = 1;

ALTER TABLE solving_analysis.subsampled_packages
ADD PRIMARY KEY (from_id, to_id);

ANALYZE solving_analysis.subsampled_packages;

GRANT SELECT ON solving_analysis.subsampled_packages TO data_analyzer;
GRANT ALL ON solving_analysis.subsampled_packages TO pinckney;
GRANT ALL ON solving_analysis.subsampled_packages TO federico;