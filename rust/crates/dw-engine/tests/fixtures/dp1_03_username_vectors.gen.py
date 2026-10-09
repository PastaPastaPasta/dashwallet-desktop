import json, sys
PIN = "dashpay/platform@bc41f1bc233dec4607d387101c1d9c2f111019b2"
Q = f"{PIN}:packages/dash-platform-queries/src/dpns_usernames.rs"
S = f"{PIN}:packages/rs-sdk/tests/dpns_unit_tests.rs"
F = f"{PIN}:packages/rs-sdk-ffi/src/dpns/helpers.rs (dash_sdk_dpns_get_validation_message)"
C = f"{PIN}:packages/dpns-contract/schema/v1/dpns-contract-documents.json"
IOS = "dashpay/dashwallet-ios@37c0e78:DashWallet/Sources/UI/DashPay/DWDashPayConstants.m (DW_MAX_USERNAME_LENGTH = 23); DASHPAY.md §2.9"
validity = []  # (label, dpns_valid, source)
def v(label, ok, src): validity.append((label, ok, src))
# dash-platform-queries test_is_valid_username
for l in ["abc","alice","Alice123","dash-p2p","test-name-123","a-b-c","user2024","CryptoKing","web3-developer","a"*63]:
    v(l, True, Q+" test_is_valid_username")
for l in ["ab","a","","a"*64,"-alice","-test","alice-","test-","-alice-","alice_bob","alice.bob","alice@dash","alice!","alice bob","alice#1","alice$","alice%20","alice--bob","test---name"]:
    v(l, False, Q+" test_is_valid_username")
# rs-sdk dpns_unit_tests
for l,ok in [("alice",True),("test",True),("dash",True),("a",False),("ab",False),("123",True),("test-name",True),("test--name",False),("-test",False),("test-",False)]:
    v(l, ok, S+" test_dpns_validation_functions")
for l in ["test_name","test.name","test@name","test name","test/name","test\\name","test:name","test;name","test'name","test\"name"]:
    v(l, False, S+" test_dpns_edge_cases (special characters)")
for l in ["café","münchen","北京","🚀rocket","user₿"]:
    v(l, False, S+" test_dpns_edge_cases (unicode)")
# contract schema label pattern ^[a-zA-Z0-9][a-zA-Z0-9-]{0,61}[a-zA-Z0-9]$, minLength 3
v("a"*23, True, C+" domain.label pattern; 23 = desktop cap ("+IOS+")")
v("a"*24, True, C+" domain.label pattern; 24 > desktop cap ("+IOS+")")

# homograph folding (convert_to_homograph_safe_chars)
normalize = []
def n(label, out, src): normalize.append((label,out,src))
for a,b in [("alice","a11ce"),("bob","b0b"),("COOL","c001"),("test123","test123")]:
    n(a,b,Q+" test_convert_to_homograph_safe_chars")
for a,b in [("alice","a11ce"),("bob","b0b"),("COOL","c001"),("test123","test123"),("ali","a11"),("dash","dash")]:
    n(a,b,S+" test_dpns_validation_functions")
for a,b in [("paypal","paypa1"),("google","g00g1e"),("microsoft","m1cr0s0ft"),("admin","adm1n"),("root","r00t"),("alice","a11ce"),("bill","b111"),("cool","c001"),("lol","101"),("oil","011")]:
    n(a,b,S+" test_dpns_homograph_safety")
for a,b in [("hello","he110"),("world","w0r1d"),("dash-dao","dash-da0"),("a11ce","a11ce")]:
    n(a,b,Q+" test_is_contested_username (comments)")

contested = []
def c(label, out, src): contested.append((label,out,src))
for l in ["abc","alice","b0b","cool","a-b-c","hello","world","dash","a11ce","dash-dao"]:
    c(l, True, Q+" test_is_contested_username")
for l in ["ab","io","a","twenty-characters-ab","this-is-a-very-long-username-that-exceeds-limit","alice2","alice_bob","alice.bob","alice@dash","alice!","test123","dash-p2p","user5","name_with_underscore"]:
    c(l, False, Q+" test_is_contested_username")
for l,o in [("abc",True),("test",True),("alice",True),("Alice",True),("test-name",True),("test123",False),("a",False),("ab",False),("twentycharacterslong",False)]:
    c(l, o, S+" test_dpns_validation_functions")
c("a"*19, True, C+" domain index parentNameAndLabel contested regex ^[a-zA-Z01-]{3,19}$")
c("a"*20, False, C+" domain index parentNameAndLabel contested regex ^[a-zA-Z01-]{3,19}$")

# the rule each invalid label breaks: the categories of dash_sdk_dpns_get_validation_message
rules = [
  ("ab", ["min_length"], F+" 'at least 3 characters'"),
  ("a"*24, ["max_length"], IOS),
  ("-alice", ["no_edge_hyphen"], F+" 'must start with an alphanumeric character'"),
  ("alice-", ["no_edge_hyphen"], F+" 'must end with an alphanumeric character'"),
  ("alice--bob", ["no_double_hyphen"], F+" 'cannot contain consecutive hyphens'"),
  ("alice_bob", ["allowed_characters"], F+" 'only contain letters, numbers, and hyphens'"),
  ("café", ["allowed_characters"], S+" test_dpns_edge_cases (unicode); length counts characters"),
  ("-", ["min_length","no_edge_hyphen"], F),
  ("", ["min_length"], Q+" test_is_valid_username"),
]
# homograph collisions: labels that are one DPNS name (same normalizedLabel)
collisions = [
  (["alice","Alice","ALICE","a11ce","A1ICE","aIice"], "a11ce", S+" test_dpns_validation_functions ('Alice' contested as 'a11ce') + "+Q+" ('a11ce already normalized'); "+C+" normalizedLabel $comment"),
  (["bob","B0B","b0b","BOB"], "b0b", Q+" test_convert_to_homograph_safe_chars"),
  (["cool","COOL","c001","CO0L"], "c001", Q+" test_convert_to_homograph_safe_chars"),
  (["lol","101","LOL","l0I"], "101", S+" test_dpns_homograph_safety"),
  (["paypal","paypa1","PAYPAL"], "paypa1", S+" test_dpns_homograph_safety"),
]
out = {
  "about": "DP1-03 username rule vectors. Every expectation is an assertion of the cited upstream test or rule at the pin; 'valid' adds only the desktop's 23-character cap (DASHPAY §2.9) to the upstream verdict 'dpns_valid'. In 'homograph_collisions' the first label of each set and the normalized form are the cited test's literals; the other spellings apply the folding rule of convert_to_homograph_safe_chars (o/O->0, i/I/l/L->1, lower case), stated in the cited sources, to that literal. Generated by dp1_03_username_vectors.gen.py next to this file (python3 -I dp1_03_username_vectors.gen.py dp1_03_username_vectors.json).",
  "pin": PIN,
  "max_length": 23,
  "validity": [{"label":l,"dpns_valid":ok,"valid": ok and len(l)<=23,"source":s} for l,ok,s in validity],
  "normalize": [{"label":l,"normalized":o,"source":s} for l,o,s in normalize],
  "contested": [{"label":l,"contested":o,"source":s} for l,o,s in contested],
  "broken_rules": [{"label":l,"rules":r,"source":s} for l,r,s in rules],
  "homograph_collisions": [{"labels":ls,"normalized":nn,"source":s} for ls,nn,s in collisions],
}
json.dump(out, open(sys.argv[1],"w"), ensure_ascii=False, indent=1)
