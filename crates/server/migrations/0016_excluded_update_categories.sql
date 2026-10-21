INSERT INTO setting (key, value)
SELECT
    'user.' || user_id || '.library.excluded_update_categories',
    json_group_array(id)
FROM category
WHERE include_in_update = 2
GROUP BY user_id
ON CONFLICT (key) DO NOTHING;
