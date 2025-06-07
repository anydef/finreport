-- Add up migration script here
alter table categories
    add constraint unique_category_subcategory unique (category, subcategory);