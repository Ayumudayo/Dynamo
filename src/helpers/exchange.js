const axios = require("axios");

const API_KEY = process.env.EXCHANGE_API_KEY;

/**
 * 두 통화 간의 환율을 가져오거나, 지정한 금액에 대한 변환 결과를 반환.
 * @param {string} from - 기준 통화 코드 (예: USD)
 * @param {string} to - 대상 통화 코드 (예: KRW)
 * @param {number} [amount=1] - 변환할 금액. 생략하면 1 단위 환율(conversion_rate)만 반환.
 * @returns {Promise<number>} - 변환 결과 또는 환율 (숫자)
 */
async function convert(from, to, amount = 1) {
  const url = `https://v6.exchangerate-api.com/v6/${API_KEY}/pair/${from}/${to}/${amount}`;
  try {
    const response = await axios.get(url);
    const json = response.data;

    if (json.result === "success") {
      return json.conversion_result !== undefined ? json.conversion_result : json.conversion_rate;
    } else {
      throw new Error(json["error-type"] || "Unknown error from ExchangeRate-API");
    }
  } catch (error) {
    if (error.response) {
      const apiError = error.response.data?.["error-type"] || "Unknown API Error";
      throw new Error(`ExchangeRate-API Error: ${apiError}`);
    } else if (error.request) {
      throw new Error("No response from ExchangeRate-API server.");
    } else {
      throw new Error(`Error setting up request: ${error.message}`);
    }
  }
}

/**
 * 두 통화 간의 1단위 환율(conversion_rate)만 가져온다.
 * @param {string} from - 기준 통화 코드
 * @param {string} to - 대상 통화 코드
 * @returns {Promise<number>} - 환율 (숫자)
 */
function getRate(from, to) {
  return convert(from, to, 1);
}

/**
 * 기준 통화에서 여러 대상 통화로 변환 결과를 병렬로 가져온온다.
 * @param {string} from - 기준 통화 코드
 * @param {number} amount - 변환할 금액
 * @param {string[]} targetCurrencies - 대상 통화 코드 배열
 * @returns {Promise<Array<{ currency: string, rate: number }>>}
 */
function getRates(from, amount, targetCurrencies) {
  const promises = targetCurrencies.map((to) => {
    return convert(from, to, amount).then((rate) => ({ currency: to, rate }));
  });
  return Promise.all(promises);
}

module.exports = {
  convert,
  getRate,
  getRates,
};
