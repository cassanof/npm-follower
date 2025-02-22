
CREATE TABLE tarball_analysis.what_did_updates_change AS
SELECT 
    u.from_id,
    u.to_id,
    u.package_id,
    u.ty,
    u.from_created,
    u.to_created,
    ROW(u.from_id, u.to_id) IN (SELECT from_id, to_id FROM metadata_analysis.vuln_intro_updates) as did_intro_vuln,
    ROW(u.from_id, u.to_id) IN (SELECT from_id, to_id FROM metadata_analysis.vuln_patch_updates) as did_patch_vuln,
    f.did_change_types, 
    f.did_change_code, 
    d.did_add_dep, 
    d.did_remove_dep, 
    d.did_change_dep_constraint as did_modify_dep_constraint, 
    s.did_change_json_scripts
FROM metadata_analysis.all_updates u
INNER JOIN tarball_analysis.diff_changed_files f ON f.from_id = u.from_id AND f.to_id = u.to_id
INNER JOIN metadata_analysis.update_dep_changes d ON u.from_id = d.from_id AND u.to_id = d.to_id
INNER JOIN metadata_analysis.update_did_change_json_scripts s ON u.from_id = s.from_id AND u.to_id = s.to_id;


GRANT SELECT ON tarball_analysis.what_did_updates_change TO data_analyzer;
GRANT ALL ON tarball_analysis.what_did_updates_change TO pinckney;
GRANT ALL ON tarball_analysis.what_did_updates_change TO federico;

ALTER TABLE tarball_analysis.what_did_updates_change
ADD PRIMARY KEY (from_id, to_id);

ANALYZE tarball_analysis.what_did_updates_change;



