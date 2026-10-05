// Line-protocol driver around Dash Core's own src/wallet/bip39.cpp, so the
// vectors in testdata/bip39_core_quirks.json record what Dash Core computes.
// Built and driven by make_vectors.py; links Core's bip39.cpp, sha256.cpp,
// sha512.cpp, hmac_sha512.cpp, pkcs5_pbkdf2_hmac_sha512.cpp, lockedpool.cpp
// and cleanse.cpp from a Dash Core checkout.
//
// Input lines (fields separated by a tab, strings hex-encoded):
//   CHECK <mnemonic-hex>                 -> "1" or "0"
//   SEED <mnemonic-hex> <passphrase-hex> -> 64-byte seed hex
//   FROMDATA <entropy-hex>               -> mnemonic text
#include <wallet/bip39.h>

#include <span.h>

#include <cstdio>
#include <iostream>
#include <sstream>
#include <string>
#include <vector>

// CMnemonic::Generate references this; the oracle never calls Generate.
void GetStrongRandBytes(Span<unsigned char>) noexcept { abort(); }

static std::string unhex(const std::string& h)
{
    std::string out;
    for (size_t i = 0; i + 1 < h.size(); i += 2) out.push_back(char(std::stoi(h.substr(i, 2), nullptr, 16)));
    return out;
}

static std::string hex(const unsigned char* p, size_t n)
{
    static const char* d = "0123456789abcdef";
    std::string s;
    for (size_t i = 0; i < n; ++i) {
        s.push_back(d[p[i] >> 4]);
        s.push_back(d[p[i] & 15]);
    }
    return s;
}

int main()
{
    std::string line;
    while (std::getline(std::cin, line)) {
        std::vector<std::string> f;
        std::stringstream ss(line);
        std::string part;
        while (std::getline(ss, part, '\t')) f.push_back(part);
        if (f.empty()) continue;
        if (f[0] == "CHECK") {
            std::string m = f.size() > 1 ? unhex(f[1]) : "";
            std::cout << (CMnemonic::Check(SecureString(m.begin(), m.end())) ? "1" : "0") << "\n";
        } else if (f[0] == "SEED") {
            std::string m = f.size() > 1 ? unhex(f[1]) : "";
            std::string p = f.size() > 2 ? unhex(f[2]) : "";
            SecureVector seed;
            CMnemonic::ToSeed(SecureString(m.begin(), m.end()), SecureString(p.begin(), p.end()), seed);
            std::cout << hex(seed.data(), seed.size()) << "\n";
        } else if (f[0] == "FROMDATA") {
            std::string e = unhex(f[1]);
            SecureVector data(e.begin(), e.end());
            SecureString m = CMnemonic::FromData(data, data.size());
            std::cout << std::string(m.begin(), m.end()) << "\n";
        } else {
            std::cerr << "bad command " << f[0] << "\n";
            return 2;
        }
        std::cout.flush();
    }
    return 0;
}
