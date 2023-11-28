ALTER TABLE contract_verification_zksolc_versions DROP CONSTRAINT contract_verification_zksolc_versions_pkey;
ALTER TABLE contract_verification_solc_versions DROP CONSTRAINT contract_verification_solc_versions_pkey;
CREATE INDEX witness_inputs_block_number_idx ON witness_inputs USING btree (l1_batch_number);
ALTER TABLE witness_inputs ADD CONSTRAINT unique_witnesses UNIQUE (l1_batch_number);
ALTER TABLE witness_inputs DROP CONSTRAINT witness_inputs_pkey;