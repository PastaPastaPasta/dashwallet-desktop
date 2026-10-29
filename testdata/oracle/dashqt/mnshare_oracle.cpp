// dash-qt's shared-masternode session envelope as an oracle (MG-04b M9).
//
// Links Dash Core v24.0.0-rc.3's own src/qt/mnsharesession.cpp, unmodified,
// and drives MnShareSession on regtest:
//
//   mnshare_oracle stages   <cases.json>  : for each case, builds the session
//       the way the creation dialog does (shares and terms, then one
//       addContribution per participant, freeze, addSignature per share,
//       setCombinedTx, setFundingSignedTx, setBroadcast) and prints
//       toJsonString() and fingerprint() after every step;
//   mnshare_oracle verdicts <corpus.json> : prints fromJson's verdict on each
//       text, its error, whether the fingerprint matched, and toJsonString()
//       of the session it read.
//
// Both print JSON on stdout. gen_envelopes.py writes the inputs and turns the
// output into testdata/shared_envelopes.json.

#include <qt/mnsharesession.h>

#include <bls/bls.h>
#include <chainparams.h>
#include <chainparamsbase.h>
#include <crypto/sha256.h>
#include <key.h>
#include <random.h>
#include <univalue.h>
#include <util/translation.h>

#include <QCoreApplication>

#include <fstream>
#include <functional>
#include <iostream>
#include <sstream>
#include <string>

// Every Core program defines it; nothing here is translated.
const std::function<std::string(const char*)> G_TRANSLATION_FUN = nullptr;

namespace {

[[noreturn]] void Fail(const std::string& what, const QString& error = {})
{
    std::cerr << "mnshare_oracle: " << what;
    if (!error.isEmpty()) std::cerr << ": " << error.toStdString();
    std::cerr << std::endl;
    std::exit(1);
}

UniValue ReadJson(const char* path)
{
    std::ifstream file{path, std::ios::binary};
    if (!file) Fail(std::string{"cannot read "} + path);
    std::stringstream text;
    text << file.rdbuf();
    UniValue json;
    if (!json.read(text.str())) Fail(std::string{path} + " is not JSON");
    return json;
}

QString Q(const UniValue& v) { return QString::fromStdString(v.get_str()); }

UniValue Step(const std::string& step, const MnShareSession& session)
{
    UniValue ret(UniValue::VOBJ);
    ret.pushKV("step", step);
    ret.pushKV("text", session.toJsonString().toStdString());
    ret.pushKV("fingerprint", session.fingerprint().toStdString());
    return ret;
}

UniValue RunCase(const UniValue& c)
{
    const std::string name{c["name"].get_str()};
    UniValue steps(UniValue::VARR);
    QString error;

    // A fixed session id: the constructor draws a random one, so the empty
    // session is re-read with the case's id.
    MnShareSession session;
    {
        UniValue json{session.toJson()};
        json.pushKV("sessionId", c["sessionId"].get_str());
        if (!session.fromJson(json, error)) Fail(name + ": setting the session id", error);
    }
    steps.push_back(Step("new", session));

    for (const UniValue& s : c["shares"].getValues()) {
        MnShareSession::Share share;
        share.label = Q(s["label"]);
        share.amount = s["amount"].getInt<int64_t>();
        share.ownerAddress = Q(s["ownerAddress"]);
        share.refundAddress = Q(s["refundAddress"]);
        share.rewardAddress = Q(s["rewardAddress"]);
        session.shares().push_back(share);
    }
    const UniValue& t{c["terms"]};
    MnShareSession::Terms& terms{session.terms()};
    terms.coreP2PAddrs = Q(t["coreP2PAddrs"]);
    terms.operatorPubKey = Q(t["operatorPubKey"]);
    terms.votingAddress = Q(t["votingAddress"]);
    terms.operatorReward = t["operatorReward"].getInt<int>();
    terms.earlyPeriodBlocks = t["earlyPeriodBlocks"].getInt<uint32_t>();
    terms.earlyPenalty = t["earlyPenalty"].getInt<int64_t>();
    session.setPrepareWallet(Q(c["prepareWallet"]));
    session.setOperatorSecretHolder(Q(c["operatorSecretHolder"]));
    session.setCoordinatorLabel(Q(c["coordinatorLabel"]));
    session.noteDraftChange();
    steps.push_back(Step("terms", session));

    for (const UniValue& k : c["contributions"].getValues()) {
        MnShareSession::Contribution contribution;
        contribution.label = Q(k["label"]);
        for (const UniValue& i : k["inputs"].getValues()) {
            MnShareSession::Input input;
            input.txid = Q(i["txid"]);
            input.vout = i["vout"].getInt<uint32_t>();
            input.sequence = i["sequence"].getInt<uint32_t>();
            contribution.inputs.push_back(input);
        }
        if (const UniValue& change{k["change"]}; change.isObject()) {
            contribution.hasChange = true;
            contribution.changeAddress = Q(change["address"]);
            contribution.changeAmount = change["amount"].getInt<int64_t>();
        }
        if (!session.addContribution(contribution, error)) Fail(name + ": addContribution", error);
        steps.push_back(Step("contribution " + contribution.label.toStdString(), session));
    }

    if (!session.freeze(Q(c["protx"]), Q(c["consentHash"]), c["collateralIndex"].getInt<int>(), error)) {
        Fail(name + ": freeze", error);
    }
    steps.push_back(Step("freeze", session));

    for (const UniValue& sig : c["signatures"].getValues()) {
        const int index{sig["shareIndex"].getInt<int>()};
        if (!session.addSignature(index, Q(sig["signature"]), error)) Fail(name + ": addSignature", error);
        steps.push_back(Step("signature " + std::to_string(index), session));
    }
    if (!session.setCombinedTx(Q(c["combinedTx"]), error)) Fail(name + ": setCombinedTx", error);
    steps.push_back(Step("combine", session));
    if (!session.setFundingSignedTx(Q(c["fundingSignedTx"]), error)) Fail(name + ": setFundingSignedTx", error);
    steps.push_back(Step("sign funding", session));
    if (!session.setBroadcast(error)) Fail(name + ": setBroadcast", error);
    steps.push_back(Step("broadcast", session));

    UniValue ret(UniValue::VOBJ);
    ret.pushKV("name", name);
    ret.pushKV("steps", steps);
    return ret;
}

UniValue Verdict(const UniValue& entry)
{
    MnShareSession session;
    QString error;
    const bool ok{session.fromJson(entry["text"].get_str(), error)};
    UniValue ret(UniValue::VOBJ);
    ret.pushKV("name", entry["name"].get_str());
    ret.pushKV("ok", ok);
    if (ok) {
        ret.pushKV("fingerprint_ok", session.importWarning().isEmpty());
        ret.pushKV("text", session.toJsonString().toStdString());
    } else {
        ret.pushKV("error", error.toStdString());
    }
    return ret;
}

} // namespace

int main(int argc, char** argv)
{
    QCoreApplication app{argc, argv};
    if (argc != 3) Fail("usage: mnshare_oracle stages|verdicts <input.json>");
    SHA256AutoDetect();
    RandomInit();
    ECC_Start();
    BLSInit();
    SelectParams(CBaseChainParams::REGTEST);

    const std::string mode{argv[1]};
    const UniValue input{ReadJson(argv[2])};
    UniValue out(UniValue::VARR);
    for (const UniValue& entry : input.getValues()) {
        if (mode == "stages") {
            out.push_back(RunCase(entry));
        } else if (mode == "verdicts") {
            out.push_back(Verdict(entry));
        } else {
            Fail("unknown mode " + mode);
        }
    }
    std::cout << out.write(2) << std::endl;
    ECC_Stop();
    return 0;
}
