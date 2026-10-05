// Golden-vector generator for dash-qt URI and amount formatting.
//
// The functions in the `core` namespace are copied from Dash Core develop
// (src/qt/guiutil.cpp parseBitcoinURI/formatBitcoinURI/formatAmount and
// src/qt/bitcoinunits.cpp). They are copied, not reimplemented, so that the
// JSON this program writes records what dash-qt itself does when linked
// against Qt 5.15 (Dash Core depends pins Qt 5.15.18).
//
// Copyright (c) 2011-2021 The Bitcoin Core developers
// Copyright (c) 2014-2025 The Dash Core developers
// Distributed under the MIT software license.
//
// Build and run: testdata/oracle/qt/run.sh

#include <QCoreApplication>
#include <QFile>
#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QStringList>
#include <QUrl>
#include <QUrlQuery>

#include <cstdint>
#include <cstdio>
#include <optional>

typedef int64_t CAmount;

#define THIN_SP_CP 0x2009

namespace core {

static bool g_mainnet = true;

enum class Unit { DASH, mDASH, uDASH, duffs };
enum class SeparatorStyle { NEVER, STANDARD, ALWAYS };

static constexpr auto MAX_DIGITS_BTC = 16;

QString name(Unit unit)
{
    const bool is_mainnet{g_mainnet};
    switch (unit) {
    case Unit::DASH:  return is_mainnet ? QString("DASH") : QString("tDASH");
    case Unit::mDASH: return is_mainnet ? QString("mDASH") : QString("mtDASH");
    case Unit::uDASH: return is_mainnet ? QString::fromUtf8("μDASH") : QString::fromUtf8("μtDASH");
    case Unit::duffs: return is_mainnet ? QString("duffs") : QString("tduffs");
    }
    abort();
}

qint64 factor(Unit unit)
{
    switch (unit) {
    case Unit::DASH:  return 100'000'000;
    case Unit::mDASH: return 100'000;
    case Unit::uDASH: return 100;
    case Unit::duffs: return 1;
    }
    abort();
}

int decimals(Unit unit)
{
    switch (unit) {
    case Unit::DASH:  return 8;
    case Unit::mDASH: return 5;
    case Unit::uDASH: return 2;
    case Unit::duffs: return 0;
    }
    abort();
}

QString format(Unit unit, const CAmount& nIn, bool fPlus, SeparatorStyle separators, bool justify = false)
{
    qint64 n = (qint64)nIn;
    qint64 coin = factor(unit);
    int num_decimals = decimals(unit);
    qint64 n_abs = (n > 0 ? n : -n);
    qint64 quotient = n_abs / coin;
    QString quotient_str = QString::number(quotient);
    if (justify) {
        quotient_str = quotient_str.rightJustified(MAX_DIGITS_BTC - num_decimals, ' ');
    }
    QChar thin_sp(THIN_SP_CP);
    int q_size = quotient_str.size();
    if (separators == SeparatorStyle::ALWAYS || (separators == SeparatorStyle::STANDARD && q_size > 4))
        for (int i = 3; i < q_size; i += 3)
            quotient_str.insert(q_size - i, thin_sp);

    if (n < 0)
        quotient_str.insert(0, '-');
    else if (fPlus && n > 0)
        quotient_str.insert(0, '+');

    if (num_decimals > 0) {
        qint64 remainder = n_abs % coin;
        QString remainder_str = QString::number(remainder).rightJustified(num_decimals, '0');
        return quotient_str + QString(".") + remainder_str;
    } else {
        return quotient_str;
    }
}

QString formatWithUnit(Unit unit, const CAmount& amount, bool plussign, SeparatorStyle separators)
{
    return format(unit, amount, plussign, separators) + QString(" ") + name(unit);
}

QString formatWithPrivacy(Unit unit, const CAmount& amount, SeparatorStyle separators, bool privacy)
{
    QString value;
    if (privacy) {
        value = format(unit, 0, false, separators, true).replace('0', '#');
    } else {
        value = format(unit, amount, false, separators, true);
    }
    return value + QString(" ") + name(unit);
}

// The QSettings "digits" read is replaced by a parameter.
QString floorWithUnit(Unit unit, const CAmount& amount, bool plussign, SeparatorStyle separators, int digits)
{
    QString result = format(unit, amount, plussign, separators);
    if (decimals(unit) > digits) result.chop(decimals(unit) - digits);
    return result + QString(" ") + name(unit);
}

QString removeSpaces(QString text)
{
    text.remove(' ');
    text.remove(QChar(THIN_SP_CP));
    return text;
}

bool parse(Unit unit, const QString& value, CAmount* val_out)
{
    if (value.isEmpty()) {
        return false;
    }
    int num_decimals = decimals(unit);
    QStringList parts = removeSpaces(value).split(".");
    if (parts.size() > 2) {
        return false;
    }
    const QString& whole = parts[0];
    QString decimals;
    if (parts.size() > 1) {
        decimals = parts[1];
    }
    if (decimals.size() > num_decimals) {
        return false;
    }
    bool ok = false;
    QString str = whole + decimals.leftJustified(num_decimals, '0');
    if (str.size() > 18) {
        return false;
    }
    CAmount retvalue(str.toLongLong(&ok));
    if (val_out) {
        *val_out = retvalue;
    }
    return ok;
}

QString formatAmount(Unit unit, CAmount amount, bool is_signed, std::optional<uint8_t> truncate)
{
    QString formatted = format(unit, amount, is_signed, SeparatorStyle::ALWAYS);
    if (truncate) {
        int dotIndex = formatted.indexOf('.');
        if (dotIndex != -1) {
            if (*truncate == 0) {
                formatted = formatted.left(dotIndex);
            } else if (formatted.length() > dotIndex + 1 + *truncate) {
                formatted = formatted.left(dotIndex + 1 + *truncate);
            }
        }
    }
    return formatted + " " + name(unit);
}

struct SendCoinsRecipient {
    QString address;
    QString label;
    CAmount amount{0};
    QString message;
};

bool parseBitcoinURI(const QUrl& uri, SendCoinsRecipient* out)
{
    if (!uri.isValid() || uri.scheme() != QString("dash"))
        return false;

    SendCoinsRecipient rv;
    rv.address = uri.path();
    if (rv.address.endsWith("/")) {
        rv.address.truncate(rv.address.length() - 1);
    }
    rv.amount = 0;

    QUrlQuery uriQuery(uri);
    QList<QPair<QString, QString>> items = uriQuery.queryItems();

    for (QList<QPair<QString, QString>>::iterator i = items.begin(); i != items.end(); i++) {
        bool fShouldReturnFalse = false;
        if (i->first.startsWith("req-")) {
            i->first.remove(0, 4);
            fShouldReturnFalse = true;
        }
        if (i->first == "label") {
            rv.label = i->second;
            fShouldReturnFalse = false;
        }
        if (i->first == "IS") {
            fShouldReturnFalse = false;
        }
        if (i->first == "message") {
            rv.message = i->second;
            fShouldReturnFalse = false;
        } else if (i->first == "amount") {
            if (!i->second.isEmpty()) {
                if (!parse(Unit::DASH, i->second, &rv.amount)) {
                    return false;
                }
            }
            fShouldReturnFalse = false;
        }
        if (fShouldReturnFalse)
            return false;
    }
    if (out) {
        *out = rv;
    }
    return true;
}

bool parseBitcoinURI(QString uri, SendCoinsRecipient* out)
{
    QUrl uriInstance(uri);
    return parseBitcoinURI(uriInstance, out);
}

QString formatBitcoinURI(const SendCoinsRecipient& info)
{
    QString ret = QString("dash:%1").arg(info.address);
    int paramCount = 0;
    if (info.amount) {
        ret += QString("?amount=%1").arg(format(Unit::DASH, info.amount, false, SeparatorStyle::NEVER));
        paramCount++;
    }
    if (!info.label.isEmpty()) {
        QString lbl(QUrl::toPercentEncoding(info.label));
        ret += QString("%1label=%2").arg(paramCount == 0 ? "?" : "&").arg(lbl);
        paramCount++;
    }
    if (!info.message.isEmpty()) {
        QString msg(QUrl::toPercentEncoding(info.message));
        ret += QString("%1message=%2").arg(paramCount == 0 ? "?" : "&").arg(msg);
        paramCount++;
    }
    return ret;
}

} // namespace core

static const char* unitName(core::Unit u)
{
    switch (u) {
    case core::Unit::DASH: return "DASH";
    case core::Unit::mDASH: return "mDASH";
    case core::Unit::uDASH: return "uDASH";
    case core::Unit::duffs: return "duffs";
    }
    abort();
}

static const char* sepName(core::SeparatorStyle s)
{
    switch (s) {
    case core::SeparatorStyle::NEVER: return "never";
    case core::SeparatorStyle::STANDARD: return "standard";
    case core::SeparatorStyle::ALWAYS: return "always";
    }
    abort();
}

static QJsonValue amountJson(CAmount a)
{
    // JSON doubles are exact up to 2^53; every amount used here is below that
    // in magnitude, so a number is safe. Larger values are emitted as strings.
    if (a > (1LL << 53) || a < -(1LL << 53)) return QJsonValue(QString::number(a));
    return QJsonValue(double(a));
}

static QJsonDocument readJson(const char* path)
{
    QFile f(QString::fromUtf8(path));
    if (!f.open(QIODevice::ReadOnly)) {
        fprintf(stderr, "cannot open %s\n", path);
        exit(1);
    }
    QJsonParseError err;
    QJsonDocument doc = QJsonDocument::fromJson(f.readAll(), &err);
    if (err.error != QJsonParseError::NoError) {
        fprintf(stderr, "%s: %s\n", path, qPrintable(err.errorString()));
        exit(1);
    }
    return doc;
}

static void writeJson(const char* path, const QJsonObject& obj)
{
    QFile f(QString::fromUtf8(path));
    if (!f.open(QIODevice::WriteOnly | QIODevice::Truncate)) {
        fprintf(stderr, "cannot write %s\n", path);
        exit(1);
    }
    f.write(QJsonDocument(obj).toJson(QJsonDocument::Indented));
}

static QJsonObject uriCases(const QJsonObject& in)
{
    QJsonArray parseOut;
    for (const QJsonValue& v : in.value("parse").toArray()) {
        const QString input = v.toString();
        core::SendCoinsRecipient rv;
        const bool ok = core::parseBitcoinURI(input, &rv);
        QJsonObject o;
        o["input"] = input;
        o["ok"] = ok;
        if (ok) {
            o["address"] = rv.address;
            o["label"] = rv.label;
            o["message"] = rv.message;
            o["amount"] = amountJson(rv.amount);
        }
        parseOut.append(o);
    }
    QJsonArray formatOut;
    for (const QJsonValue& v : in.value("format").toArray()) {
        const QJsonObject c = v.toObject();
        core::SendCoinsRecipient rv;
        rv.address = c.value("address").toString();
        rv.label = c.value("label").toString();
        rv.message = c.value("message").toString();
        rv.amount = CAmount(c.value("amount").toDouble());
        QJsonObject o = c;
        o["uri"] = core::formatBitcoinURI(rv);
        // Round trip through the parser, as dash-qt's receive dialog output
        // would be read by another dash-qt.
        core::SendCoinsRecipient back;
        const bool ok = core::parseBitcoinURI(o["uri"].toString(), &back);
        QJsonObject rt;
        rt["ok"] = ok;
        if (ok) {
            rt["address"] = back.address;
            rt["label"] = back.label;
            rt["message"] = back.message;
            rt["amount"] = amountJson(back.amount);
        }
        o["reparsed"] = rt;
        formatOut.append(o);
    }
    QJsonObject out;
    out["source"] = "Dash Core develop src/qt/guiutil.cpp parseBitcoinURI/formatBitcoinURI, run against Qt " QT_VERSION_STR;
    out["generator"] = "testdata/oracle/qt/qt_oracle.cpp";
    out["parse"] = parseOut;
    out["format"] = formatOut;
    return out;
}

static QJsonObject amountCases(const QJsonObject& in)
{
    const core::Unit units[] = {core::Unit::DASH, core::Unit::mDASH, core::Unit::uDASH, core::Unit::duffs};
    const core::SeparatorStyle seps[] = {core::SeparatorStyle::NEVER, core::SeparatorStyle::STANDARD, core::SeparatorStyle::ALWAYS};

    std::vector<CAmount> amounts;
    for (const QJsonValue& v : in.value("amounts").toArray()) amounts.push_back(CAmount(v.toDouble()));

    QJsonArray fmt, fmtUnit, floorOut, privacyOut, fmtAmount, parseOut;
    for (core::Unit u : units) {
        for (CAmount a : amounts) {
            for (core::SeparatorStyle s : seps) {
                for (bool plus : {false, true}) {
                    for (bool justify : {false, true}) {
                        QJsonObject o;
                        o["unit"] = unitName(u);
                        o["amount"] = amountJson(a);
                        o["separators"] = sepName(s);
                        o["plus"] = plus;
                        o["justify"] = justify;
                        o["out"] = core::format(u, a, plus, s, justify);
                        fmt.append(o);
                    }
                }
            }
            for (bool mainnet : {true, false}) {
                core::g_mainnet = mainnet;
                QJsonObject o;
                o["unit"] = unitName(u);
                o["amount"] = amountJson(a);
                o["network"] = mainnet ? "main" : "test";
                o["separators"] = "standard";
                o["plus"] = false;
                o["out"] = core::formatWithUnit(u, a, false, core::SeparatorStyle::STANDARD);
                fmtUnit.append(o);
                if (a >= 0) {
                    for (bool privacy : {false, true}) {
                        QJsonObject p;
                        p["unit"] = unitName(u);
                        p["amount"] = amountJson(a);
                        p["network"] = mainnet ? "main" : "test";
                        p["separators"] = "always";
                        p["privacy"] = privacy;
                        p["out"] = core::formatWithPrivacy(u, a, core::SeparatorStyle::ALWAYS, privacy);
                        privacyOut.append(p);
                    }
                }
            }
            core::g_mainnet = true;
            for (int digits : {2, 3, 4, 8}) {
                for (bool plus : {false, true}) {
                    QJsonObject o;
                    o["unit"] = unitName(u);
                    o["amount"] = amountJson(a);
                    o["network"] = "main";
                    o["separators"] = "always";
                    o["plus"] = plus;
                    o["digits"] = digits;
                    o["out"] = core::floorWithUnit(u, a, plus, core::SeparatorStyle::ALWAYS, digits);
                    floorOut.append(o);
                }
            }
            for (bool is_signed : {false, true}) {
                for (int t : {-1, 0, 2, 9}) {
                    std::optional<uint8_t> trunc;
                    if (t >= 0) trunc = uint8_t(t);
                    QJsonObject o;
                    o["unit"] = unitName(u);
                    o["amount"] = amountJson(a);
                    o["network"] = "main";
                    o["signed"] = is_signed;
                    o["truncate"] = t >= 0 ? QJsonValue(t) : QJsonValue(QJsonValue::Null);
                    o["out"] = core::formatAmount(u, a, is_signed, trunc);
                    fmtAmount.append(o);
                }
            }
        }
        for (const QJsonValue& v : in.value("parse").toArray()) {
            const QString input = v.toString();
            CAmount val = 0;
            const bool ok = core::parse(u, input, &val);
            QJsonObject o;
            o["unit"] = unitName(u);
            o["input"] = input;
            o["ok"] = ok;
            if (ok) o["value"] = amountJson(val);
            parseOut.append(o);
        }
    }

    QJsonObject out;
    out["source"] = "Dash Core develop src/qt/bitcoinunits.cpp + GUIUtil::formatAmount, run against Qt " QT_VERSION_STR;
    out["generator"] = "testdata/oracle/qt/qt_oracle.cpp";
    out["thin_space"] = QString(QChar(THIN_SP_CP));
    out["format"] = fmt;
    out["format_with_unit"] = fmtUnit;
    out["format_with_privacy"] = privacyOut;
    out["floor_with_unit"] = floorOut;
    out["format_amount"] = fmtAmount;
    out["parse"] = parseOut;
    return out;
}

int main(int argc, char** argv)
{
    QCoreApplication app(argc, argv);
    if (argc != 5) {
        fprintf(stderr, "usage: %s uri_inputs.json uri_cases.json amount_inputs.json amount_format.json\n", argv[0]);
        return 2;
    }
    writeJson(argv[2], uriCases(readJson(argv[1]).object()));
    writeJson(argv[4], amountCases(readJson(argv[3]).object()));
    return 0;
}
