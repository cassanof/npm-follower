CREATE TABLE metadata_analysis.possible_direct_dev_deps AS WITH deps_of_package AS (
    SELECT package_id,
        unnest(dev_dependencies) AS dep_id
    FROM versions
)
SELECT DISTINCT src_dep.package_id AS pkg,
    d.dst_package_id_if_exists AS depends_on_pkg
FROM deps_of_package src_dep
    INNER JOIN dependencies d ON src_dep.dep_id = d.id
WHERE d.dst_package_id_if_exists IS NOT NULL;

ALTER TABLE metadata_analysis.possible_direct_dev_deps
ADD PRIMARY KEY (pkg, depends_on_pkg);

ALTER TABLE metadata_analysis.possible_direct_dev_deps
ADD CONSTRAINT metadata_analysis_possible_direct_dev_deps_fkey_pkg FOREIGN KEY (pkg) REFERENCES packages (id);
ALTER TABLE metadata_analysis.possible_direct_dev_deps
ADD CONSTRAINT metadata_analysis_possible_direct_dev_deps_fkey_depends_on_pkg FOREIGN KEY (depends_on_pkg) REFERENCES packages (id);


ANALYZE metadata_analysis.possible_direct_dev_deps;


GRANT SELECT ON metadata_analysis.possible_direct_dev_deps TO data_analyzer;
GRANT ALL ON metadata_analysis.possible_direct_dev_deps TO pinckney;
GRANT ALL ON metadata_analysis.possible_direct_dev_deps TO federico;
