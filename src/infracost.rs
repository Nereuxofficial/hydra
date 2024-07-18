use reqwest::header::HeaderMap;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Data {
    data: DataContent,
}

#[derive(Debug, Deserialize)]
struct DataContent {
    products: Vec<Product>,
}

#[derive(Debug, Deserialize)]
struct Product {
    region: String,
    attributes: Vec<Attribute>,
    sku: String,
    prices: Vec<Price>,
}

#[derive(Debug, Deserialize)]
struct Attribute {
    key: String,
    value: String,
}

#[derive(Debug, Deserialize)]
struct Price {
    #[serde(rename = "EUR", deserialize_with = "parse_price")]
    eur: f64,
    #[serde(rename = "purchaseOption")]
    purchase_option: InstanceType,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Deserialize)]
enum InstanceType {
    #[serde(rename = "on_demand")]
    OnDemand,
    #[serde(rename = "preemptible")]
    Preemptible,
    #[serde(rename = "spot")]
    Spot,
}

fn parse_price<'de, D>(deserializer: D) -> Result<f64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    s.parse::<f64>().map_err(serde::de::Error::custom)
}

async fn get_machine_prices() {
    let ic_api_key = std::env::var("INFRACOST_API_KEY").unwrap();
    let client = reqwest::Client::new();
    let mut headers = HeaderMap::new();
    headers.insert("X-Api-Key", ic_api_key.parse().unwrap());
    headers.insert("content-type", "application/json".parse().unwrap());
    let res = client.post("https://pricing.api.infracost.io/graphql")
        .headers(headers)
        .body("{\"query\":\"{\\n  products(\\n    filter: {vendorName: \\\"gcp\\\", service: \\\"Compute Engine\\\", productFamily: \\\"Compute Instance\\\", region: \\\"europe-west1\\\"}\\n  ) {\\n    region\\n    attributes {\\n      key\\n      value\\n    }\\n    sku\\n    prices(filter: {}) {\\n      EUR\\n      purchaseOption\\n    }\\n  }\\n}\\n\"}")
        .send().await.unwrap()
        .text().await.unwrap();
    let deserialized: Data = serde_json::from_str(&res).unwrap();
    let products = deserialized.data.products;
    let multiple_types = products.into_iter().filter(|p| {
        p.prices
            .iter()
            .any(|price| price.purchase_option == InstanceType::OnDemand)
            && p.prices
                .iter()
                .any(|price| price.purchase_option == InstanceType::Preemptible)
    });
    for product in multiple_types {
        println!(
            "{}: Price difference {}%",
            product.sku,
            price_difference(&product.prices)
        );
    }
}

fn price_difference(prices: &[Price]) -> f64 {
    let on_demand = prices
        .iter()
        .find(|price| price.purchase_option == InstanceType::OnDemand)
        .unwrap();
    let spot = prices
        .iter()
        .find(|price| price.purchase_option == InstanceType::Preemptible)
        .unwrap();
    ((on_demand.eur - spot.eur) / on_demand.eur) * 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_get_machine_prices() {
        dotenvy::dotenv().ok();
        get_machine_prices().await;
    }

    #[tokio::test]
    async fn test_infracost_aws() {}
}
