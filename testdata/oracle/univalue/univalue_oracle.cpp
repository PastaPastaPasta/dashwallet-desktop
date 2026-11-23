// UniValue oracle for MG-04b (dw-protx::shared::json, D7): Dash Core
// v24.0.0-rc.3's own src/univalue reads each input and writes it back, so
// the Rust reader and writer are compared with UniValue's behaviour, not a
// description of it. Build and run with gen_vectors.py (see README.md).
//
// stdin: one case per line, "." then the input bytes as hex (so an empty
// input is a line of its own).
// stdout: a JSON array; per case
//   input       the input (hex)
//   ok          UniValue::read's result
//   write0      write() of the value (hex), when ok
//   write2      write(2) of the value (hex), when ok
//   without_fp  write(2) of WithoutFingerprint(value) (hex), when ok: the
//               bytes dash-qt's message code hashes
//   fingerprint EnvelopeFingerprint(value), when ok and an object
//   ints        for an array, per element: getInt<int64_t>, getInt<uint64_t>,
//               getInt<int>, getInt<uint32_t> as decimal strings, or null
//               where getInt throws or the element is not a number

#include <crypto/sha256.h>
#include <univalue.h>

#include <cstdint>
#include <iostream>
#include <stdexcept>
#include <string>
#include <vector>

namespace {

// Copied verbatim from rc.3 src/qt/mnsharesession.cpp:46-57.
const char* FINGERPRINT_KEY{"fingerprint"};
UniValue WithoutFingerprint(const UniValue& json)
{
    if (!json.isObject()) return json;
    UniValue ret(UniValue::VOBJ);
    const std::vector<std::string>& keys{json.getKeys()};
    const std::vector<UniValue>& values{json.getValues()};
    for (size_t i = 0; i < keys.size(); ++i) {
        if (keys[i] == FINGERPRINT_KEY) continue;
        ret.pushKV(keys[i], values[i]);
    }
    return ret;
}

// rc.3 src/qt/mnsharesession.cpp:225-232, with std::string for QString and
// an upper-case hex loop for HexStr(…).toUpper().
std::string EnvelopeFingerprint(const UniValue& json)
{
    const std::string text{WithoutFingerprint(json).write(/*prettyIndent=*/2)};
    unsigned char digest[CSHA256::OUTPUT_SIZE];
    CSHA256().Write(reinterpret_cast<const unsigned char*>(text.data()), text.size()).Finalize(digest);
    static const char* d = "0123456789ABCDEF";
    std::string hex;
    for (int i = 0; i < 4; ++i) {
        hex += d[digest[i] >> 4];
        hex += d[digest[i] & 15];
    }
    return hex.substr(0, 4) + '-' + hex.substr(4);
}

std::string Hex(const std::string& s)
{
    static const char* d = "0123456789abcdef";
    std::string out;
    for (unsigned char c : s) {
        out += d[c >> 4];
        out += d[c & 15];
    }
    return out;
}

std::string Unhex(const std::string& h)
{
    std::string out;
    for (size_t i = 0; i + 1 < h.size(); i += 2) {
        out += static_cast<char>(std::stoi(h.substr(i, 2), nullptr, 16));
    }
    return out;
}

template <typename Int>
UniValue TryInt(const UniValue& v)
{
    try {
        return UniValue(std::to_string(v.getInt<Int>()));
    } catch (const std::runtime_error&) {
        return UniValue();
    }
}

} // namespace

int main()
{
    UniValue cases(UniValue::VARR);
    std::string line;
    while (std::getline(std::cin, line)) {
        if (line.empty() || line[0] != '.') continue;
        const std::string hex{line.substr(1)};
        const std::string input{Unhex(hex)};
        UniValue c(UniValue::VOBJ);
        c.pushKV("input", hex);
        UniValue v;
        const bool ok{v.read(input)};
        c.pushKV("ok", ok);
        if (ok) {
            c.pushKV("write0", Hex(v.write()));
            c.pushKV("write2", Hex(v.write(2)));
            c.pushKV("without_fp", Hex(WithoutFingerprint(v).write(2)));
            if (v.isObject()) c.pushKV("fingerprint", EnvelopeFingerprint(v));
            if (v.isArray()) {
                UniValue ints(UniValue::VARR);
                for (const UniValue& e : v.getValues()) {
                    UniValue row(UniValue::VARR);
                    if (e.isNum()) {
                        row.push_back(TryInt<int64_t>(e));
                        row.push_back(TryInt<uint64_t>(e));
                        row.push_back(TryInt<int>(e));
                        row.push_back(TryInt<uint32_t>(e));
                    }
                    ints.push_back(row);
                }
                c.pushKV("ints", ints);
            }
        }
        cases.push_back(c);
    }
    std::cout << cases.write(1) << "\n";
}
