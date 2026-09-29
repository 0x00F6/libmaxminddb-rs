#[allow(dead_code)]
#[derive(Debug, libmaxminddb_rs::MmdbDecode)]
pub(crate) struct Names<'a> {
    pub(crate) en: Option<&'a str>,
}

#[allow(dead_code)]
#[derive(Debug, libmaxminddb_rs::MmdbDecode)]
pub(crate) struct Continent<'a> {
    pub(crate) code: Option<&'a str>,
    pub(crate) geoname_id: Option<u32>,
    pub(crate) names: Option<Names<'a>>,
}

#[allow(dead_code)]
#[derive(Debug, libmaxminddb_rs::MmdbDecode)]
pub(crate) struct Country<'a> {
    pub(crate) geoname_id: Option<u32>,
    pub(crate) iso_code: Option<&'a str>,
    pub(crate) names: Option<Names<'a>>,
}

#[allow(dead_code)]
#[derive(Debug, libmaxminddb_rs::MmdbDecode)]
pub(crate) struct Subdivision<'a> {
    pub(crate) geoname_id: Option<u32>,
    pub(crate) iso_code: Option<&'a str>,
    pub(crate) names: Option<Names<'a>>,
}

#[allow(dead_code)]
#[derive(Debug, libmaxminddb_rs::MmdbDecode)]
pub(crate) struct City<'a> {
    pub(crate) geoname_id: Option<u32>,
    pub(crate) names: Option<Names<'a>>,
}

#[allow(dead_code)]
#[derive(Debug, libmaxminddb_rs::MmdbDecode)]
pub(crate) struct Location<'a> {
    pub(crate) accuracy_radius: Option<u16>,
    pub(crate) latitude: Option<f64>,
    pub(crate) longitude: Option<f64>,
    pub(crate) time_zone: Option<&'a str>,
}

#[allow(dead_code)]
#[derive(Debug, libmaxminddb_rs::MmdbDecode)]
pub(crate) struct CityRecord<'a> {
    pub(crate) continent: Option<Continent<'a>>,
    pub(crate) country: Option<Country<'a>>,
    pub(crate) subdivisions: Option<Vec<Subdivision<'a>>>,
    pub(crate) city: Option<City<'a>>,
    pub(crate) location: Option<Location<'a>>,
    pub(crate) registered_country: Option<Country<'a>>,
}

// Shared by sequential/concurrent comparison and the Criterion replay.
#[inline(always)]
pub(crate) fn lookup<'a>(
    reader: &'a libmaxminddb_rs::Reader<'_>,
    ip: std::net::IpAddr,
) -> libmaxminddb_rs::Result<CityRecord<'a>> {
    reader.lookup_borrowed(ip)
}
