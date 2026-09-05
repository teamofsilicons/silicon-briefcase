-- An IAM testing environment is one security plane and may back exactly one
-- Briefcase testing environment at a time. Root-key rotation changes the IAM
-- digest, so the public immutable IAM UUID is the durable one-to-one binding.
CREATE UNIQUE INDEX testing_environments_iam_environment_uidx
    ON briefcase.testing_environments (iam_environment_id);
