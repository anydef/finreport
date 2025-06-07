-- Add up migration script here
CREATE TABLE transactions
(
    reference           varchar PRIMARY KEY,
    account_id          varchar,
    booking_status      varchar          NOT NULL,
    booking_date        varchar          NOT NULL,
    amount              double precision NOT NULL,
    remitter            varchar,
    deptor              varchar,
    creditor            varchar,
    creditor_id         varchar,
    creditor_mandate_id varchar,
    remittance_info     varchar          NOT NULL,
    transaction_type    varchar          NOT NULL
);

CREATE TABLE categories
(
    id          int PRIMARY KEY,
    category    varchar,
    subcategory varchar
);


CREATE TABLE IF NOT EXISTS transaction_categories
(
    reference   varchar primary key ,
    category_id int,
    reasoning   varchar,
    confidence  double precision,
    FOREIGN KEY (reference) REFERENCES transactions (reference),
    FOREIGN key (category_id) REFERENCES categories (id)
);