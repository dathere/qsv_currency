extern crate currency;

use currency::Currency;
use std::cmp::Ordering;

#[test]
fn eq_works() {
    let a = Currency(Some('$'), 1210);
    let b = Currency(Some('$'), 1210);
    let c = Currency(Some('$'), 1251);
	
    assert!(a == b);
    assert!(b == b);
    assert!(b == a);
    assert!(a != c);
}
 
#[test]
fn ord_works() {
    let a = Currency(Some('$'), 1210);
    let b = Currency(Some('$'), 1211);
    let c = Currency(Some('$'), 1311);
    let d = Currency(Some('$'), 1210);
	
    assert_eq!(a.partial_cmp(&b), Some(Ordering::Less));
    assert_eq!(a.partial_cmp(&c), Some(Ordering::Less));
    assert_eq!(a.partial_cmp(&d), Some(Ordering::Equal));
    assert_eq!(c.partial_cmp(&a), Some(Ordering::Greater));
	
    assert!(a < b);
    assert!(a < c);
    assert!(a <= a);
    assert!(a <= c);
    assert!(b > a);
    assert!(c > a);
    assert!(a >= a);
    assert!(c >= a);
}
 
#[test]
fn arithmetic_works() {
    let x = Currency(Some('$'), 1206);
    let y = Currency(Some('$'), 1143);
    
    assert!(x + y == Currency(Some('$'), 2349)
         && y + x == Currency(Some('$'), 2349));
    assert!(x - y == Currency(Some('$'), 63));
    assert!(y - x == Currency(Some('$'), -63));
    assert!(x * 2 == Currency(Some('$'), 2412)
         && 2 * x == Currency(Some('$'), 2412));
    assert!(x / 2 == Currency(Some('$'), 603));
}
 
#[test]
fn parse_works() {
    let a = Currency(Some('$'), 1210);
    let b = Currency::from_string("$12.10");
    assert!(a == b);
    
    let c = Currency(Some('$'), 1200);
    let d = Currency::from_string("$12");
    assert!(c == d);
}
 
#[test]
fn display_works() {
    assert!(Currency(Some('$'), 1210).to_string() == "$12.10");
    assert!(Currency(None, 1210).to_string() == "12.10");
}