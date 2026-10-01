# Disposable key fixtures

These RSA-2048 keys were generated for the key-import tests with Python cryptography,
then serialized as PKCS#1, PKCS#8, OpenSSH and PuTTY v2. They are synthetic test data,
never provider or user credentials. The separate `rsa.pub` file is the independently
serialized verification oracle; container comments are outside the key identity.

The encrypted fixtures use the public test passphrase `fixture-passphrase`.
The PKCS#1 envelopes cover AES-128-CBC (OpenSSL) and AES-256-CBC (cryptography).
The DSA-1024 fixture exercises the unsupported-key error.
